use std::sync::Mutex;
use std::time::{Duration, Instant};

use koko_proto::{ControlState, HumanResult};
use tokio::sync::oneshot;

/// An agent's input is refused this long after a person's last input.
pub(crate) const HUMAN_GRACE: Duration = Duration::from_secs(3);
/// An agent that stops sending input loses control after this long.
pub(crate) const AGENT_IDLE: Duration = Duration::from_secs(60);

pub(crate) const PREEMPTED: &str = "preempted_by_human";
pub(crate) const AWAITING_HUMAN: &str = "awaiting_human";
pub(crate) const AWAITING_APPROVAL: &str = "awaiting_approval";

/// A person's answer to an agent's request for help, with an optional note for the agent.
pub(crate) type Answer = (HumanResult, Option<String>);

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
    help: Option<oneshot::Sender<Answer>>,
    approval: Option<oneshot::Sender<bool>>,
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
                help: None,
                approval: None,
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
            ControlState::AwaitingHuman => return Err(AWAITING_HUMAN),
            ControlState::AwaitingApproval => return Err(AWAITING_APPROVAL),
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
    /// While the agent waits for help, the person's input is the help, so it changes nothing.
    pub(crate) fn human_input(&self) {
        let mut state = self.state.lock().unwrap();
        state.human_input = Some(Instant::now());
        if let Some(approval) = state.approval.take() {
            let _ = approval.send(false);
            set(&mut state, ControlState::Paused);
        } else if state.control == ControlState::Active {
            set(&mut state, ControlState::Paused);
        }
    }

    /// The host owner's stop button or shortcut. Also blocks an idle agent from starting,
    /// and answers a pending request for help as taken over.
    pub(crate) fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some(help) = state.help.take() {
            let _ = help.send((HumanResult::TakenOver, None));
        }
        if let Some(approval) = state.approval.take() {
            let _ = approval.send(false);
        }
        set(&mut state, ControlState::Paused);
    }

    pub(crate) fn resume(&self) {
        let mut state = self.state.lock().unwrap();
        if state.control == ControlState::Paused {
            set(&mut state, ControlState::Idle);
        }
    }

    /// The agent gave control back. A pending request for help is withdrawn.
    pub(crate) fn release(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.control,
            ControlState::Active | ControlState::AwaitingHuman | ControlState::AwaitingApproval
        ) {
            state.help = None;
            state.approval = None;
            set(&mut state, ControlState::Idle);
        }
    }

    /// Forgets a pause or request when the agent's session ends, so the next session starts idle.
    pub(crate) fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        state.help = None;
        state.approval = None;
        set(&mut state, ControlState::Idle);
    }

    pub(crate) fn expire(&self) {
        let mut state = self.state.lock().unwrap();
        if state.control == ControlState::Active && state.agent_input.elapsed() >= self.idle {
            set(&mut state, ControlState::Idle);
        }
    }

    /// Starts waiting for a person. Only one request may be open, and a paused agent may not ask.
    pub(crate) fn request_help(&self) -> Result<oneshot::Receiver<Answer>, &'static str> {
        let mut state = self.state.lock().unwrap();
        match state.control {
            ControlState::Paused => return Err(PREEMPTED),
            ControlState::AwaitingHuman => return Err(AWAITING_HUMAN),
            ControlState::AwaitingApproval => return Err(AWAITING_APPROVAL),
            ControlState::Idle | ControlState::Active => {}
        }
        let (answer, receiver) = oneshot::channel();
        state.help = Some(answer);
        set(&mut state, ControlState::AwaitingHuman);
        Ok(receiver)
    }

    /// A person answered. The agent continues after `resolved`, otherwise it is paused.
    pub(crate) fn answer(&self, result: HumanResult, note: Option<String>) {
        let mut state = self.state.lock().unwrap();
        let Some(help) = state.help.take() else {
            return;
        };
        let _ = help.send((result, note));
        if result == HumanResult::Resolved {
            state.agent_input = Instant::now();
            set(&mut state, ControlState::Active);
        } else {
            set(&mut state, ControlState::Paused);
        }
    }

    /// Holds a risky input until the person decides. Only an agent in control gets this far.
    pub(crate) fn request_approval(&self) -> Result<oneshot::Receiver<bool>, &'static str> {
        let mut state = self.state.lock().unwrap();
        match state.control {
            ControlState::Active => {}
            ControlState::AwaitingApproval => return Err(AWAITING_APPROVAL),
            ControlState::AwaitingHuman => return Err(AWAITING_HUMAN),
            ControlState::Idle | ControlState::Paused => return Err(PREEMPTED),
        }
        let (decision, receiver) = oneshot::channel();
        state.approval = Some(decision);
        set(&mut state, ControlState::AwaitingApproval);
        Ok(receiver)
    }

    /// The person allowed or declined the input. Either way the agent keeps control.
    pub(crate) fn decide(&self, allow: bool) {
        let mut state = self.state.lock().unwrap();
        let Some(approval) = state.approval.take() else {
            return;
        };
        let _ = approval.send(allow);
        state.agent_input = Instant::now();
        set(&mut state, ControlState::Active);
    }

    /// Nobody decided in time; the input counts as declined and the agent keeps control.
    pub(crate) fn approval_timed_out(&self) {
        let mut state = self.state.lock().unwrap();
        if state.approval.take().is_some() {
            state.agent_input = Instant::now();
            set(&mut state, ControlState::Active);
        }
    }

    /// Nobody answered in time, so control goes back to the agent.
    pub(crate) fn help_timed_out(&self) {
        let mut state = self.state.lock().unwrap();
        if state.help.take().is_some() {
            state.agent_input = Instant::now();
            set(&mut state, ControlState::Active);
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
                ControlState::AwaitingHuman => "awaiting_human",
                ControlState::AwaitingApproval => "awaiting_approval",
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

    #[test]
    fn a_request_for_help_ends_with_each_kind_of_answer() {
        let arbiter = Arbiter::new(Duration::ZERO, Duration::from_secs(60));
        let mut answer = arbiter.request_help().unwrap();
        assert_eq!(arbiter.request_help().unwrap_err(), AWAITING_HUMAN);
        assert_eq!(arbiter.agent_input(), Err(AWAITING_HUMAN));
        // The person's input is the help itself and does not pause the agent.
        arbiter.human_input();
        assert_eq!(arbiter.control(), ControlState::AwaitingHuman);
        arbiter.answer(HumanResult::Resolved, Some("Signed in".into()));
        assert_eq!(
            answer.try_recv().unwrap(),
            (HumanResult::Resolved, Some("Signed in".into()))
        );
        assert_eq!(arbiter.control(), ControlState::Active);

        let mut answer = arbiter.request_help().unwrap();
        arbiter.answer(HumanResult::Declined, None);
        assert_eq!(answer.try_recv().unwrap().0, HumanResult::Declined);
        assert_eq!(arbiter.control(), ControlState::Paused);
        assert_eq!(arbiter.request_help().unwrap_err(), PREEMPTED);

        arbiter.resume();
        let mut answer = arbiter.request_help().unwrap();
        arbiter.stop();
        assert_eq!(answer.try_recv().unwrap().0, HumanResult::TakenOver);

        arbiter.resume();
        let _answer = arbiter.request_help().unwrap();
        arbiter.help_timed_out();
        assert_eq!(arbiter.control(), ControlState::Active);
    }

    #[test]
    fn a_risky_input_waits_for_the_person() {
        let arbiter = Arbiter::new(Duration::ZERO, Duration::from_secs(60));
        assert_eq!(arbiter.request_approval().unwrap_err(), PREEMPTED);
        arbiter.agent_input().unwrap();

        let mut decision = arbiter.request_approval().unwrap();
        assert_eq!(arbiter.agent_input(), Err(AWAITING_APPROVAL));
        arbiter.decide(true);
        assert_eq!(decision.try_recv(), Ok(true));
        assert_eq!(arbiter.control(), ControlState::Active);

        let mut decision = arbiter.request_approval().unwrap();
        arbiter.decide(false);
        assert_eq!(decision.try_recv(), Ok(false));
        assert_eq!(arbiter.control(), ControlState::Active);

        // A person who acts while a decision is pending takes over instead.
        let mut decision = arbiter.request_approval().unwrap();
        arbiter.human_input();
        assert_eq!(decision.try_recv(), Ok(false));
        assert_eq!(arbiter.control(), ControlState::Paused);
    }
}
