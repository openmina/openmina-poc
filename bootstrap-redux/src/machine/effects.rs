use mina_p2p_messages::core::Info;
use redux::{Store, ActionWithMeta};

use super::{
    state::State,
    action::Action,
    rpc::{
        Action as RpcAction, OutgoingAction as RpcOutgoingAction, Request as RpcRequest, Message,
        Response,
    },
    sync_ledger::Action as SyncLedgerAction,
};
use crate::Service;

pub fn run(store: &mut Store<State, Service, Action>, action: ActionWithMeta<Action>) {
    match action.action() {
        Action::RpcRawBytes {
            peer_id,
            connection_id,
            ..
        } => {
            let msgs = store.state().last_responses.clone();
            for msg in &msgs {
                match msg {
                    Message::Heartbeat => {
                        store.dispatch(Action::Rpc(RpcAction::Heartbeat {
                            peer_id: *peer_id,
                            connection_id: *connection_id,
                        }));
                    }
                    Message::Response {
                        body: Response::BestTip(b),
                        ..
                    } => {
                        let Ok(v) = &b.0 else {
                            log::error!("get best tip failed");
                            return;
                        };
                        let Some(v) = &v.0 else {
                            log::warn!("best tip is none");
                            return;
                        };

                        // TODO:
                        // let mut peers = store.state().rpc.outgoing.keys();
                        // let (peer_id, connection_id) = peers.next().unwrap();
                        // let q = vec![v.data.header.delta_block_chain_proof.0 .0.clone()];
                        // store.dispatch(RpcAction::Outgoing {
                        //     peer_id: *peer_id,
                        //     connection_id: *connection_id,
                        //     inner: RpcOutgoingAction::Init(RpcRequest::GetTransitionChain(q)),
                        // });
                        store.dispatch(SyncLedgerAction::Start(v.clone()));
                    }
                    Message::Response {
                        body: Response::GetTransitionChainProof(v),
                        ..
                    } => {
                        let v = serde_json::to_string(&v.0.as_ref().unwrap().0.as_ref().unwrap())
                            .unwrap();
                        log::info!("{v}");
                    }
                    Message::Response {
                        body: Response::GetTransitionChain(v),
                        ..
                    } => {
                        let v = serde_json::to_string(&v.0.as_ref().unwrap().0.as_ref().unwrap())
                            .unwrap();
                        log::info!("{v}");
                    }
                    Message::Response {
                        body: Response::SyncLedger(b),
                        ..
                    } => {
                        let Ok(v) = &b.0 else {
                            log::error!("sync ledger failed");
                            return;
                        };
                        match &v.0 .0 {
                            Err(err) => {
                                if let Info::CouldNotConstruct(s) = err {
                                    log::warn!("sync ledger failed {}", s.to_string_lossy());
                                } else {
                                    log::warn!("sync ledger failed {err:?}")
                                }
                                store.dispatch(SyncLedgerAction::Continue(None));
                            }
                            Ok(v) => {
                                store.dispatch(SyncLedgerAction::Continue(Some(v.clone())));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Action::RpcNegotiated {
            peer_id,
            connection_id,
        } => {
            store.dispatch(Action::Rpc(RpcAction::Outgoing {
                peer_id: *peer_id,
                connection_id: *connection_id,
                inner: RpcOutgoingAction::Init(RpcRequest::BestTip(())),
            }));
        }
        Action::Rpc(inner) => inner.clone().effects(action.meta(), store),
        Action::SyncLedger(inner) => inner.clone().effects(action.meta(), store),
        _ => {}
    }
}
