use libp2p::PeerId;
use serde::{Serialize, Deserialize};

use super::{state::State, rpc::Action as RpcAction};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Action {
    GossipMessage,
    RpcNegotiated {
        peer_id: PeerId,
        connection_id: usize,
    },
    RpcRawBytes {
        peer_id: PeerId,
        connection_id: usize,
        bytes: Vec<u8>,
    },
    RpcClosed {
        peer_id: PeerId,
        connection_id: usize,
    },
    Rpc(RpcAction),
}

impl From<RpcAction> for Action {
    fn from(value: RpcAction) -> Self {
        Action::Rpc(value)
    }
}

impl redux::EnablingCondition<State> for Action {
    fn is_enabled(&self, state: &State) -> bool {
        match self {
            Action::Rpc(inner) => inner.is_enabled(&state.rpc),
            Action::RpcRawBytes { bytes, .. } => !bytes.is_empty(),
            _ => true,
        }
    }
}
