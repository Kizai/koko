use iroh::EndpointId;
use koko_proto::{PeerKind, Scope};

/// The controllers currently allowed to use this host: at most one person and one agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Sessions {
    pub(crate) human: Option<EndpointId>,
    pub(crate) agent: Option<(EndpointId, Scope)>,
}

impl Sessions {
    #[cfg(test)]
    pub(crate) fn human(peer: EndpointId) -> Self {
        Self {
            human: Some(peer),
            agent: None,
        }
    }

    pub(crate) fn scope(&self, peer: EndpointId) -> Option<Scope> {
        if self.human == Some(peer) {
            return Some(Scope::Human);
        }
        self.agent
            .filter(|(agent, _)| *agent == peer)
            .map(|(_, scope)| scope)
    }

    pub(crate) fn end(&mut self, peer: EndpointId) -> Option<PeerKind> {
        if self.human == Some(peer) {
            self.human = None;
            Some(PeerKind::Human)
        } else if self.agent.is_some_and(|(agent, _)| agent == peer) {
            self.agent = None;
            Some(PeerKind::Agent)
        } else {
            None
        }
    }
}

pub(crate) fn report_end(kind: PeerKind) {
    println!(
        "{}",
        match kind {
            PeerKind::Human => "Session closed",
            PeerKind::Agent => "Agent session closed",
        }
    );
}
