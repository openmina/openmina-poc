use mina_p2p_messages::v2;
use redux::{Store, ActionMeta};

use super::{
    Action,
    super::rpc::{Action as RpcAction, OutgoingAction as RpcOutgoingAction, Request as RpcRequest},
};
use crate::{
    service::Service,
    machine::{State as GlobalState, Action as GlobalAction},
};

impl Action {
    pub fn effects(self, _: &ActionMeta, store: &mut Store<GlobalState, Service, GlobalAction>) {
        match self {
            Action::Start(_) => {
                let ledger_hash = store
                    .state()
                    .sync_ledger
                    .epoch_ledger_hash
                    .as_ref()
                    .expect("enabling conditions");
                log::info!("Synchronizing Ledger: {ledger_hash}");

                // TODO: choose most suitable peer
                let mut peers = store.state().rpc.outgoing.keys();
                let (peer_id, connection_id) = peers.next().unwrap();
                let q = (
                    ledger_hash.0.clone(),
                    v2::MinaLedgerSyncLedgerQueryStableV1::NumAccounts,
                );
                store.dispatch(RpcAction::Outgoing {
                    peer_id: *peer_id,
                    connection_id: *connection_id,
                    inner: RpcOutgoingAction::Init(RpcRequest::SyncLedger(q)),
                });
            }
            Action::Continue(v) => {
                // TODO: add action
                if let Some(v2::MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(accounts)) = v {
                    store
                        .service()
                        .ledger_storage
                        .add_accounts(accounts)
                        .unwrap();
                }

                let ledger_hash = store
                    .state()
                    .sync_ledger
                    .epoch_ledger_hash
                    .as_ref()
                    .expect("enabling conditions")
                    .0
                    .clone();
                let depth = store.state().sync_ledger.syncing_depth;
                if depth > 32 {
                    // TODO:
                    store.service().ledger_storage.root_hash();
                    return;
                }
                let pos = store.state().sync_ledger.syncing_pos;
                let pos = pos.to_be_bytes()[..((depth as usize + 7) / 8)].to_vec();

                log::info!("perform query, depth: {depth}, pos: {}", hex::encode(&pos));
                let query = if depth < 32 {
                    (
                        ledger_hash,
                        v2::MinaLedgerSyncLedgerQueryStableV1::WhatChildHashes(
                            v2::MerkleAddressBinableArgStableV1(depth.into(), pos.into()),
                        ),
                    )
                } else if depth == 32 {
                    (
                        ledger_hash,
                        v2::MinaLedgerSyncLedgerQueryStableV1::WhatContents(
                            v2::MerkleAddressBinableArgStableV1(depth.into(), pos.into()),
                        ),
                    )
                } else {
                    return;
                };

                // TODO: choose most suitable peer
                let mut peers = store.state().rpc.outgoing.keys();
                let (peer_id, connection_id) = peers.next().unwrap();
                store.dispatch(RpcAction::Outgoing {
                    peer_id: *peer_id,
                    connection_id: *connection_id,
                    inner: RpcOutgoingAction::Init(RpcRequest::SyncLedger(query)),
                });
            }
        }
    }
}
