use std::sync::Mutex;
use std::time::{Duration, Instant};

use koko_proto::ControlState;

/// An agent's input is refused this long after a person's last input.
pub(crate) const HUMAN_GRACE: Duration = Duration::from_secs(3);
/// An agent that stops sending input loses control after this long.
pub(crate) const AGENT_IDLE: Duration = Duration::from_secs(60);

pub(crate) const PREEMPTED: &str = "preempted_by_human";

/// Decides whether the connected agent may send input. People always win.
#[derive(Debug)]
pub(crate) struct Arbiter {
    grace: Duration,
    idle: Duration,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    control: ControlState,
    human_input: Option<Instant>,
    agent_input: Instant,
}

impl Arbiter {
    pub(crate) fn new(grace: Duration, idle: Duration) -> Self {
        Self {
            grace,
            idle,
            state: Mutex::new(State {
                control: ControlState::Idle,
                human_input: None,
                agent_input: Instant::now(),
            }),
        }
    }

    pub(crate) fn control(&self) -> ControlState {
        self.state.lock().unwrap().control
    }

    /// Takes control for the agent if nobody objects.
    pub(crate) fn agent_input(&self) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();
        match state.control {
            ControlState::Paused => return Err(PREEMPTED),
            ControlState::Idle => {
                if state
                    .human_input
                    .is_some_and(|time| time.elapsed() < self.grace)
                {
                    return Err(PREEMPTED);
                }
                set(&mut state, ControlState::Active);
            }
            ControlState::Active => {}
        }
        state.agent_input = Instant::now();
        Ok(())
    }

    /// A person's input pauses an active agent until a person lets it continue.
    pub(crate) fn human_input(&self) {
        let mut state = self.state.lock().unwrap();
        state.human_input = Some(Instant::now());
        if state.control == ControlState::Active {
            set(&mut state, ControlState::Paused);
        }
    }

    /// The host owner's stop button or shortcut. Also blocks an idle agent from starting.
    pub(crate) fn stop(&self) {
        set(&mut self.state.lock().unwrap(), ControlState::Paused);
    }

    pub(crate) fn resume(&self) {
        let mut state = self.state.lock().unwrap();
        if state.control == ControlState::Paused {
            set(&mut state, ControlState::Idle);
        }
    }

    /// The agent gave control back or its session ended.
    pub(crate) fn release(&self) {
        let mut state = self.state.lock().unwrap();
        if state.control == ControlState::Active {
            set(&mut state, ControlState::Idle);
        }
    }

    /// Forgets a pause when the agent's session ends, so the next agent session starts idle.
    pub(crate) fn reset(&self) {
        set(&mut self.state.lock().unwrap(), ControlState::Idle);
    }

    pub(crate) fn expire(&self) {
        let mut state = self.state.lock().unwrap();
        if state.control == ControlState::Active && state.agent_input.elapsed() >= self.idle {
            set(&mut state, ControlState::Idle);
        }
    }
}

fn set(state: &mut State, control: ControlState) {
    if state.control != control {
        state.control = control;
        println!(
            "Agent control: {}",
            match control {
                ControlState::Idle => "idle",
                ControlState::Active => "active",
                ControlState::Paused => "paused",
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_take_over_and_decide_when_the_agent_continues() {
        let arbiter = Arbiter::new(Duration::from_secs(60), Duration::from_secs(60));
        assert_eq!(arbiter.agent_input(), Ok(()));
        assert_eq!(arbiter.control(), ControlState::Active);

        arbiter.human_input();
        assert_eq!(arbiter.control(), ControlState::Paused);
        assert_eq!(arbiter.agent_input(), Err(PREEMPTED));

        // Resuming returns to idle, but recent human input still holds the agent back.
        arbiter.resume();
        assert_eq!(arbiter.control(), ControlState::Idle);
        assert_eq!(arbiter.agent_input(), Err(PREEMPTED));
    }

    #[test]
    fn stop_blocks_an_idle_agent_and_idle_control_expires() {
        let arbiter = Arbiter::new(Duration::ZERO, Duration::ZERO);
        arbiter.stop();
        assert_eq!(arbiter.agent_input(), Err(PREEMPTED));
        arbiter.resume();
        assert_eq!(arbiter.agent_input(), Ok(()));
        arbiter.expire();
        assert_eq!(arbiter.control(), ControlState::Idle);
        arbiter.agent_input().unwrap();
        arbiter.release();
        assert_eq!(arbiter.control(), ControlState::Idle);
    }
}
