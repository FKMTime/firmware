#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub enum Scene {
    Connecting,
    WaitingForCompetitor {
        time: Option<u64>,
    },
    CompetitorInfo(/* Competitor info struct? */),
    Inspection {
        start_time: u64,
    },
    Timer {
        inspection_time: u64,
    },
    Finished {
        inspection_time: u64,
        solve_time: u64,
    },
    Error {
        msg: alloc::string::String,
    },
}
