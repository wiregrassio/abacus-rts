//! Abacus RTS client SDK.
//!
//! Connect to the daemon, create/attach interlocks, WaitTimer, WaitCounter,
//! background touch thread, RTSTimeout/RTSOverrun handling.

pub mod client;
pub mod interlock;
pub mod process_clock;
pub mod touch;
pub mod types;
pub mod wait_barrier;
pub mod wait_counter;
pub mod wait_cron;
pub mod wait_race;
pub mod wait_timer;

pub use client::{AbacusClient, SdkError};
pub use interlock::{AttachedInterlock, AttachedWaitCounter, ClockHandle, Interlock};
pub use process_clock::ProcessClock;
pub use touch::TouchThread;
pub use types::{InterlockState, WaitResult, WaitState, WatchedWord, RTS_TIMEOUT, RTS_OVERRUN};
pub use wait_barrier::WaitBarrier;
pub use wait_counter::WaitCounter;
pub use wait_cron::WaitCron;
pub use wait_race::WaitRace;
pub use wait_timer::WaitTimer;
