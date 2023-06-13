use std::{
    fs,
    collections::{BTreeSet, BTreeMap},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    path::Path,
};

mod msg {
    use std::{net::SocketAddr, time::SystemTime};
    use serde::{Serialize, Deserialize};

    #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
    pub struct ConnectionId(pub u64);

    #[derive(
        Default, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord,
    )]
    #[serde(rename_all = "snake_case")]
    pub enum StreamId {
        #[default]
        Handshake,
        Forward(u64),
        Backward(u64),
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct FullMessage {
        pub connection_id: ConnectionId,
        pub remote_addr: SocketAddr,
        pub incoming: bool,
        pub timestamp: SystemTime,
        pub stream_id: StreamId,
        pub stream_kind: String,
        // dynamic type, the type is depend on `stream_kind`
        pub message: serde_json::Value,
        pub size: u32,
    }
}
use self::msg::FullMessage;

use reqwest::{blocking::Client, Url};

fn main() {
    struct Stream {
        client: Client,
        id: u64,
        url: Url,
    }

    impl Iterator for Stream {
        type Item = Vec<(u64, FullMessage)>;

        fn next(&mut self) -> Option<Self::Item> {
            let response = self
                .client
                .get(format!("{}&id={}", self.url, self.id))
                .send()
                .ok()?;
            let page = serde_json::from_reader::<_, Vec<(u64, FullMessage)>>(response).unwrap();
            self.id = page.last().map(|x| x.0 + 1)?;
            Some(page)
        }
    }

    let client = Client::builder().build().unwrap();
    let stream = Stream {
        client: client.clone(),
        id: 0,
        url: "http://1.k8.openmina.com:30675/messages?limit=1000&stream_kind=coda/rpcs/0.0.1"
            .parse()
            .unwrap(),
    };

    let terminate = Arc::new(AtomicBool::new(false));
    {
        let terminate = terminate.clone();
        ctrlc::set_handler(move || {
            terminate.store(true, Ordering::SeqCst);
        })
        .unwrap();
    }

    let mut values = Vec::<(String, Vec<_>)>::new();
    let mut dedup = BTreeSet::new();
    let mut requests = BTreeMap::new();
    let mut last_kind = String::new();
    for (id, msg) in stream.flatten() {
        if terminate.load(Ordering::SeqCst) {
            break;
        }
        let Some(kind) = msg.message.as_str() else {
            continue;
        };
        if kind == "__Versioned_rpc.Menu" {
            continue;
        }
        if kind != last_kind {
            last_kind = kind.to_owned();
            println!("{id} {kind}");
        }
        if let "get_ancestry"
        | "get_best_tip"
        | "answer_sync_ledger_query"
        | "get_transition_chain"
        | "get_transition_chain_proof"
        | "get_staged_ledger_aux_and_pending_coinbases_at_hash" = kind
        {
            let full_msg = client
                .get(format!("http://1.k8.openmina.com:30675/message/{id}"))
                .send()
                .unwrap()
                .text()
                .unwrap();
            let full_msg = serde_json::from_str::<serde_json::Value>(&full_msg).unwrap();
            let payload = |m: &serde_json::Value, q: u8| -> Option<serde_json::Value> {
                let message = m.as_object()?.get("message")?.as_object()?;
                if q == 0 {
                    return message.get("id").cloned();
                }
                match (message.get("type")?.as_str()?, q) {
                    ("request", 1) => message.get("query").cloned(),
                    ("response", 2) => message.get("value").map(|_| serde_json::Value::Null),
                    _ => None,
                }
            };
            let Some(id) = payload(&full_msg, 0) else {
                continue;
            };
            let Some(id) = id.as_i64() else {
                continue;
            };
            if let Some(request) = payload(&full_msg, 1) {
                if dedup.insert(request.to_string()) {
                    requests.insert(id, request);
                }
            } else if let Some(_response) = payload(&full_msg, 2) {
                if let Some(request) = requests.remove(&id) {
                    let mut full_msg = full_msg;
                    let obj = full_msg.as_object_mut().unwrap();
                    obj.remove("connection_id");
                    obj.remove("stream_id");
                    obj.remove("stream_kind");
                    obj.get_mut("message")
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .insert("query".to_string(), request);
                    let c = values
                        .iter_mut()
                        .position(|(x, _)| x == kind)
                        .unwrap_or_else(|| {
                            values.push((kind.to_string(), vec![]));
                            values.len() - 1
                        });
                    values[c].1.push((id, full_msg));
                    // Termination condition
                    if "get_transition_chain" == kind && id >= 32700 {
                        break;
                    }
                }
            }
        }
    }
    let path = AsRef::<Path>::as_ref("target");
    for (name, values) in values {
        let file = fs::File::create(path.join(name)).unwrap();
        serde_json::to_writer_pretty(file, &values).unwrap();
    }
}
