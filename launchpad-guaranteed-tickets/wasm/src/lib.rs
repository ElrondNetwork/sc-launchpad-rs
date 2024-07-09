// Code generated by the multiversx-sc multi-contract system. DO NOT EDIT.

////////////////////////////////////////////////////
////////////////// AUTO-GENERATED //////////////////
////////////////////////////////////////////////////

// Init:                                 1
// Endpoints:                           41
// Async Callback (empty):               1
// Total number of exported functions:  43

#![no_std]
#![feature(lang_items)]

multiversx_sc_wasm_adapter::allocator!();
multiversx_sc_wasm_adapter::panic_handler!();

multiversx_sc_wasm_adapter::endpoints! {
    launchpad_guaranteed_tickets
    (
        upgrade
        addTickets
        depositLaunchpadTokens
        addUsersToBlacklist
        removeGuaranteedUsersFromBlacklist
        distributeGuaranteedTickets
        claimLaunchpadTokens
        claimTicketPayment
        getUserTicketsStatus
        getLaunchStageFlags
        getConfiguration
        getLaunchpadTokenId
        getLaunchpadTokensPerWinningTicket
        getTicketPrice
        getNumberOfWinningTickets
        getTotalLaunchpadTokensDeposited
        setTicketPrice
        setLaunchpadTokensPerWinningTicket
        setConfirmationPeriodStartBlock
        setWinnerSelectionStartBlock
        setClaimStartBlock
        getTicketRangeForAddress
        getTotalNumberOfTicketsForAddress
        getTotalNumberOfTickets
        getNumberOfConfirmedTicketsForAddress
        filterTickets
        selectWinners
        getNumberOfWinningTicketsForAddress
        getWinningTicketIdsForAddress
        setSupportAddress
        getSupportAddress
        removeUsersFromBlacklist
        isUserBlacklisted
        confirmTickets
        hasUserClaimedTokens
        setUnlockSchedule
        updateUnlockSchedule
        getClaimableTokens
        getUserTotalClaimableBalance
        getUserClaimedBalance
        getUnlockSchedule
    )
}

multiversx_sc_wasm_adapter::empty_callback! {}
