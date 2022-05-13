#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

mod launch_stage;
mod ongoing_operation;
mod random;
mod setup;

use crate::launch_stage::Flags;
use launch_stage::EpochsConfig;
use ongoing_operation::*;
use random::Random;
use setup::TokenAmountPair;

const FIRST_TICKET_ID: usize = 1;

type TicketStatus = bool;
const WINNING_TICKET: TicketStatus = true;

#[derive(TopEncode, TopDecode)]
pub struct TicketRange {
    pub first_id: usize,
    pub last_id: usize,
}

#[derive(TopEncode, TopDecode)]
pub struct TicketBatch<M: ManagedTypeApi> {
    pub address: ManagedAddress<M>,
    pub nr_tickets: usize,
}

#[elrond_wasm::contract]
pub trait Launchpad:
    launch_stage::LaunchStageModule + setup::SetupModule + ongoing_operation::OngoingOperationModule
{
    #[allow(clippy::too_many_arguments)]
    #[init]
    fn init(
        &self,
        launchpad_token_id: TokenIdentifier,
        launchpad_tokens_per_winning_ticket: BigUint,
        ticket_payment_token: TokenIdentifier,
        ticket_price: BigUint,
        nr_winning_tickets: usize,
        confirmation_period_start_epoch: u64,
        winner_selection_start_epoch: u64,
        claim_start_epoch: u64,
    ) {
        self.launchpad_token_id().set(&launchpad_token_id);

        self.try_set_launchpad_tokens_per_winning_ticket(&launchpad_tokens_per_winning_ticket);
        self.try_set_ticket_price(ticket_payment_token, ticket_price);
        self.try_set_nr_winning_tickets(nr_winning_tickets);

        let config = EpochsConfig {
            confirmation_period_start_epoch,
            winner_selection_start_epoch,
            claim_start_epoch,
        };
        self.require_valid_time_periods(&config);
        self.configuration().set(&config);
        self.flags().set_if_empty(&Flags {
            were_tickets_filtered: false,
            were_winners_selected: false,
            has_winner_selection_process_started: false,
        });

        let caller = self.blockchain().get_caller();
        self.support_address().set(&caller);
    }

    #[only_owner]
    #[endpoint(claimTicketPayment)]
    fn claim_ticket_payment(&self) {
        self.require_claim_period();

        let owner = self.blockchain().get_caller();

        let ticket_payment_mapper = self.claimable_ticket_payment();
        let claimable_ticket_payment = ticket_payment_mapper.get();
        if claimable_ticket_payment > 0 {
            ticket_payment_mapper.clear();

            let ticket_price: TokenAmountPair<Self::Api> = self.ticket_price().get();
            self.send().direct(
                &owner,
                &ticket_price.token_id,
                0,
                &claimable_ticket_payment,
                &[],
            );
        }

        let launchpad_token_id = self.launchpad_token_id().get();
        let launchpad_tokens_needed = self.get_exact_launchpad_tokens_needed();
        let launchpad_tokens_balance = self.blockchain().get_sc_balance(&launchpad_token_id, 0);
        let extra_launchpad_tokens = launchpad_tokens_balance - launchpad_tokens_needed;
        if extra_launchpad_tokens > 0 {
            self.send()
                .direct(&owner, &launchpad_token_id, 0, &extra_launchpad_tokens, &[]);
        }
    }

    #[endpoint(addUsersToBlacklist)]
    fn add_users_to_blacklist(&self, users_list: MultiValueEncoded<ManagedAddress>) {
        self.require_extended_permissions();
        self.require_before_winner_selection();

        let blacklist_mapper = self.blacklist();
        for address in users_list {
            let confirmed_tickets_mapper = self.nr_confirmed_tickets(&address);
            let nr_confirmed_tickets = confirmed_tickets_mapper.get();
            if nr_confirmed_tickets > 0 {
                self.refund_ticket_payment(&address, nr_confirmed_tickets);
                confirmed_tickets_mapper.clear();
            }

            blacklist_mapper.add(&address);
        }
    }

    #[endpoint(removeUsersFromBlacklist)]
    fn remove_users_from_blacklist(&self, users_list: MultiValueEncoded<ManagedAddress>) {
        self.require_extended_permissions();
        self.require_before_winner_selection();

        let blacklist_mapper = self.blacklist();
        for address in users_list {
            blacklist_mapper.remove(&address);
        }
    }

    #[only_owner]
    #[endpoint(addTickets)]
    fn add_tickets(
        &self,
        address_number_pairs: MultiValueEncoded<MultiValue2<ManagedAddress, usize>>,
    ) {
        self.require_add_tickets_period();

        for multi_arg in address_number_pairs {
            let (buyer, nr_tickets) = multi_arg.into_tuple();

            self.try_create_tickets(buyer, nr_tickets);
        }
    }

    #[only_owner]
    #[endpoint(setSupportAddress)]
    fn add_support_address(&self, address: ManagedAddress) {
        self.support_address().set(&address);
    }

    #[payable("*")]
    #[endpoint(confirmTickets)]
    fn confirm_tickets(&self, nr_tickets_to_confirm: usize) {
        let (payment_amount, payment_token) = self.call_value().payment_token_pair();

        self.require_confirmation_period();
        require!(
            self.were_launchpad_tokens_deposited(),
            "Launchpad tokens not deposited yet"
        );

        let caller = self.blockchain().get_caller();
        require!(
            !self.is_user_blacklisted(&caller),
            "You have been put into the blacklist and may not confirm tickets"
        );

        let total_tickets = self.get_total_number_of_tickets_for_address(&caller);
        let nr_confirmed = self.nr_confirmed_tickets(&caller).get();
        let total_confirmed = nr_confirmed + nr_tickets_to_confirm;
        require!(
            total_confirmed <= total_tickets,
            "Trying to confirm too many tickets"
        );

        let ticket_price: TokenAmountPair<Self::Api> = self.ticket_price().get();
        let total_ticket_price = ticket_price.amount * nr_tickets_to_confirm as u32;
        require!(
            payment_token == ticket_price.token_id,
            "Wrong payment token used"
        );
        require!(payment_amount == total_ticket_price, "Wrong amount sent");

        self.nr_confirmed_tickets(&caller).set(&total_confirmed);
    }

    #[endpoint(filterTickets)]
    fn filter_tickets(&self) -> OperationCompletionStatus {
        self.require_winner_selection_period();

        let flags_mapper = self.flags();
        let mut flags: Flags = flags_mapper.get();
        require!(!flags.were_tickets_filtered, "Tickets already filtered");

        let last_ticket_id = self.last_ticket_id().get();
        let (mut first_ticket_id_in_batch, mut nr_removed) = self.load_filter_tickets_operation();

        if first_ticket_id_in_batch == FIRST_TICKET_ID {
            flags.has_winner_selection_process_started = true;
        }

        let run_result = self.run_while_it_has_gas(|| {
            let current_ticket_batch_mapper = self.ticket_batch(first_ticket_id_in_batch);
            let ticket_batch: TicketBatch<Self::Api> = current_ticket_batch_mapper.get();
            let address = &ticket_batch.address;
            let nr_tickets_in_batch = ticket_batch.nr_tickets;

            let nr_confirmed_tickets = self.nr_confirmed_tickets(address).get();
            if self.is_user_blacklisted(address) || nr_confirmed_tickets == 0 {
                self.ticket_range_for_address(address).clear();
                current_ticket_batch_mapper.clear();
            } else if nr_removed > 0 || nr_confirmed_tickets < nr_tickets_in_batch {
                let new_first_id = first_ticket_id_in_batch - nr_removed;
                let new_last_id = new_first_id + nr_confirmed_tickets - 1;

                current_ticket_batch_mapper.clear();

                self.ticket_range_for_address(address).set(&TicketRange {
                    first_id: new_first_id,
                    last_id: new_last_id,
                });
                self.ticket_batch(new_first_id).set(&TicketBatch {
                    address: ticket_batch.address,
                    nr_tickets: nr_confirmed_tickets,
                });
            }

            nr_removed += nr_tickets_in_batch - nr_confirmed_tickets;
            first_ticket_id_in_batch += nr_tickets_in_batch;

            if first_ticket_id_in_batch == last_ticket_id + 1 {
                STOP_OP
            } else {
                CONTINUE_OP
            }
        });

        match run_result {
            OperationCompletionStatus::InterruptedBeforeOutOfGas => {
                self.save_progress(&OngoingOperationType::FilterTickets {
                    first_ticket_id_in_batch,
                    nr_removed,
                });
            }
            OperationCompletionStatus::Completed => {
                // this only happens when a lot of tickets have been eliminated,
                // and we end up with less total tickets than winning
                let new_last_ticket_id = last_ticket_id - nr_removed;
                let nr_winning_tickets = self.nr_winning_tickets().get();
                if nr_winning_tickets > new_last_ticket_id {
                    self.nr_winning_tickets().set(&new_last_ticket_id);
                }

                self.last_ticket_id().set(&new_last_ticket_id);
                flags.were_tickets_filtered = true;
            }
        };

        flags_mapper.set(&flags);

        run_result
    }

    #[endpoint(selectWinners)]
    fn select_winners(&self) -> OperationCompletionStatus {
        self.require_winner_selection_period();

        let flags_mapper = self.flags();
        let mut flags: Flags = flags_mapper.get();
        require!(flags.were_tickets_filtered, "Must filter tickets first");
        require!(!flags.were_winners_selected, "Winners already selected");

        let nr_winning_tickets = self.nr_winning_tickets().get();
        let last_ticket_position = self.get_total_tickets();

        let (mut rng, mut ticket_position) = self.load_select_winners_operation();
        let run_result = self.run_while_it_has_gas(|| {
            self.shuffle_single_ticket(&mut rng, ticket_position, last_ticket_position);

            if ticket_position == nr_winning_tickets {
                return STOP_OP;
            }

            ticket_position += 1;

            CONTINUE_OP
        });

        match run_result {
            OperationCompletionStatus::InterruptedBeforeOutOfGas => {
                let mut seed_bytes = [0u8; random::HASH_LEN];
                let _ = rng.seed.load_to_byte_array(&mut seed_bytes);

                self.save_progress(&OngoingOperationType::SelectWinners {
                    seed: ManagedByteArray::new_from_bytes(&seed_bytes),
                    seed_index: rng.index,
                    ticket_position,
                });
            }
            OperationCompletionStatus::Completed => {
                flags.were_winners_selected = true;

                let ticket_price: TokenAmountPair<Self::Api> = self.ticket_price().get();
                let claimable_ticket_payment = ticket_price.amount * (nr_winning_tickets as u32);
                self.claimable_ticket_payment()
                    .set(&claimable_ticket_payment);
            }
        };

        flags_mapper.set(&flags);

        run_result
    }

    #[endpoint(claimLaunchpadTokens)]
    fn claim_launchpad_tokens(&self) {
        self.require_claim_period();

        let caller = self.blockchain().get_caller();
        require!(!self.has_user_claimed(&caller), "Already claimed");

        let ticket_range = self.try_get_ticket_range(&caller);
        let nr_confirmed_tickets = self.nr_confirmed_tickets(&caller).get();
        let mut nr_redeemable_tickets = 0;

        for ticket_id in ticket_range.first_id..=ticket_range.last_id {
            let ticket_status = self.ticket_status(ticket_id).get();
            if ticket_status == WINNING_TICKET {
                self.ticket_status(ticket_id).clear();

                nr_redeemable_tickets += 1;
            }

            self.ticket_pos_to_id(ticket_id).clear();
        }

        self.nr_confirmed_tickets(&caller).clear();
        self.ticket_range_for_address(&caller).clear();
        self.ticket_batch(ticket_range.first_id).clear();

        if nr_redeemable_tickets > 0 {
            self.nr_winning_tickets()
                .update(|nr_winning_tickets| *nr_winning_tickets -= nr_redeemable_tickets);
        }

        self.claim_list().add(&caller);

        let nr_tickets_to_refund = nr_confirmed_tickets - nr_redeemable_tickets;
        self.refund_ticket_payment(&caller, nr_tickets_to_refund);
        self.send_launchpad_tokens(&caller, nr_redeemable_tickets);
    }

    // views

    // range is [min, max], both inclusive
    #[view(getTicketRangeForAddress)]
    fn get_ticket_range_for_address(
        &self,
        address: &ManagedAddress,
    ) -> OptionalValue<MultiValue2<usize, usize>> {
        let ticket_range_mapper = self.ticket_range_for_address(address);
        if ticket_range_mapper.is_empty() {
            return OptionalValue::None;
        }

        let ticket_range: TicketRange = ticket_range_mapper.get();
        OptionalValue::Some((ticket_range.first_id, ticket_range.last_id).into())
    }

    #[view(getTotalNumberOfTicketsForAddress)]
    fn get_total_number_of_tickets_for_address(&self, address: &ManagedAddress) -> usize {
        let ticket_range_mapper = self.ticket_range_for_address(address);
        if ticket_range_mapper.is_empty() {
            return 0;
        }

        let ticket_range: TicketRange = ticket_range_mapper.get();
        ticket_range.last_id - ticket_range.first_id + 1
    }

    #[view(getWinningTicketIdsForAddress)]
    fn get_winning_ticket_ids_for_address(
        &self,
        address: ManagedAddress,
    ) -> MultiValueEncoded<usize> {
        let flags: Flags = self.flags().get();
        let ticket_range_mapper = self.ticket_range_for_address(&address);
        let mut ticket_ids = MultiValueEncoded::new();
        if !flags.were_winners_selected || ticket_range_mapper.is_empty() {
            return ticket_ids;
        }

        let ticket_range: TicketRange = ticket_range_mapper.get();
        for ticket_id in ticket_range.first_id..=ticket_range.last_id {
            let actual_ticket_status = self.ticket_status(ticket_id).get();
            if actual_ticket_status == WINNING_TICKET {
                ticket_ids.push(ticket_id);
            }
        }

        ticket_ids
    }

    #[view(getNumberOfWinningTicketsForAddress)]
    fn get_number_of_winning_tickets_for_address(&self, address: ManagedAddress) -> usize {
        self.get_winning_ticket_ids_for_address(address).len()
    }

    // private

    fn try_create_tickets(&self, buyer: ManagedAddress, nr_tickets: usize) {
        let ticket_range_mapper = self.ticket_range_for_address(&buyer);
        require!(ticket_range_mapper.is_empty(), "Duplicate entry for user");

        let last_ticket_id_mapper = self.last_ticket_id();
        let first_ticket_id = last_ticket_id_mapper.get() + 1;
        let last_ticket_id = first_ticket_id + nr_tickets - 1;

        ticket_range_mapper.set(&TicketRange {
            first_id: first_ticket_id,
            last_id: last_ticket_id,
        });
        self.ticket_batch(first_ticket_id).set(&TicketBatch {
            address: buyer,
            nr_tickets,
        });
        last_ticket_id_mapper.set(last_ticket_id);
    }

    /// Fisher-Yates algorithm,
    /// each position i is swapped with a random one in range [i, n]
    fn shuffle_single_ticket(
        &self,
        rng: &mut Random<Self::Api>,
        current_ticket_position: usize,
        last_ticket_position: usize,
    ) {
        let rand_pos = rng.next_usize_in_range(current_ticket_position, last_ticket_position + 1);

        let winning_ticket_id = self.get_ticket_id_from_pos(rand_pos);
        self.ticket_status(winning_ticket_id).set(WINNING_TICKET);

        let current_ticket_id = self.get_ticket_id_from_pos(current_ticket_position);
        self.ticket_pos_to_id(rand_pos).set(current_ticket_id);
    }

    fn try_get_ticket_range(&self, address: &ManagedAddress) -> TicketRange {
        let ticket_range_mapper = self.ticket_range_for_address(address);
        require!(!ticket_range_mapper.is_empty(), "You have no tickets");

        ticket_range_mapper.get()
    }

    fn get_ticket_id_from_pos(&self, ticket_pos: usize) -> usize {
        let ticket_id = self.ticket_pos_to_id(ticket_pos).get();
        if ticket_id == 0 {
            ticket_pos
        } else {
            ticket_id
        }
    }

    fn require_extended_permissions(&self) {
        let caller = self.blockchain().get_caller();
        let owner = self.blockchain().get_owner_address();
        let support_address = self.support_address().get();

        require!(
            caller == owner || caller == support_address,
            "Permission denied"
        );
    }

    #[inline]
    fn get_total_tickets(&self) -> usize {
        self.last_ticket_id().get()
    }

    #[view(hasUserClaimedTokens)]
    fn has_user_claimed(&self, address: &ManagedAddress) -> bool {
        self.claim_list().contains(address)
    }

    #[view(isUserBlacklisted)]
    fn is_user_blacklisted(&self, address: &ManagedAddress) -> bool {
        self.blacklist().contains(address)
    }

    fn refund_ticket_payment(&self, address: &ManagedAddress, nr_tickets_to_refund: usize) {
        if nr_tickets_to_refund == 0 {
            return;
        }

        let ticket_price: TokenAmountPair<Self::Api> = self.ticket_price().get();
        let ticket_payment_refund_amount = ticket_price.amount * nr_tickets_to_refund as u32;
        self.send().direct(
            address,
            &ticket_price.token_id,
            0,
            &ticket_payment_refund_amount,
            &[],
        );
    }

    fn send_launchpad_tokens(&self, address: &ManagedAddress, nr_claimed_tickets: usize) {
        if nr_claimed_tickets == 0 {
            return;
        }

        let launchpad_token_id = self.launchpad_token_id().get();
        let tokens_per_winning_ticket = self.launchpad_tokens_per_winning_ticket().get();
        let launchpad_tokens_amount_to_send =
            BigUint::from(nr_claimed_tickets as u32) * tokens_per_winning_ticket;

        self.send().direct(
            address,
            &launchpad_token_id,
            0,
            &launchpad_tokens_amount_to_send,
            &[],
        );
    }

    // storage

    #[storage_mapper("ticketStatus")]
    fn ticket_status(&self, ticket_id: usize) -> SingleValueMapper<TicketStatus>;

    #[view(getTotalNumberOfTickets)]
    #[storage_mapper("lastTicketId")]
    fn last_ticket_id(&self) -> SingleValueMapper<usize>;

    #[storage_mapper("ticketBatch")]
    fn ticket_batch(&self, start_index: usize) -> SingleValueMapper<TicketBatch<Self::Api>>;

    #[storage_mapper("ticketRangeForAddress")]
    fn ticket_range_for_address(&self, address: &ManagedAddress) -> SingleValueMapper<TicketRange>;

    #[view(getNumberOfConfirmedTicketsForAddress)]
    #[storage_mapper("nrConfirmedTickets")]
    fn nr_confirmed_tickets(&self, address: &ManagedAddress) -> SingleValueMapper<usize>;

    // only used during shuffling. Default (0) means ticket pos = ticket ID.
    #[storage_mapper("ticketPosToId")]
    fn ticket_pos_to_id(&self, ticket_pos: usize) -> SingleValueMapper<usize>;

    #[storage_mapper("claimableTicketPayment")]
    fn claimable_ticket_payment(&self) -> SingleValueMapper<BigUint>;

    #[view(getSupportAddress)]
    #[storage_mapper("supportAddress")]
    fn support_address(&self) -> SingleValueMapper<ManagedAddress>;

    // flags

    #[storage_mapper("claimedTokens")]
    fn claim_list(&self) -> WhitelistMapper<Self::Api, ManagedAddress>;

    #[storage_mapper("blacklisted")]
    fn blacklist(&self) -> WhitelistMapper<Self::Api, ManagedAddress>;
}
