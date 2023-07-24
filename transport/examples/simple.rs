use mina_transport::{futures::StreamExt, OutputEvent, rpc, gossipsub, BehaviourEvent};

#[tokio::main]
async fn main() {
    let local_key = mina_transport::generate_identity();
    let peers = [
        "/ip4/35.192.28.217/tcp/10000/p2p/12D3KooWAdgYL6hv18M3iDBdaK1dRygPivSfAfBNDzie6YqydVbs"
            .parse()
            .unwrap(),
        // "/dns4/seed-2.berkeley.o1test.net/tcp/10001/p2p/12D3KooWLjs54xHzVmMmGYb7W5RVibqbwD1co7M2ZMfPgPm7iAag".parse().unwrap(),
        // "/dns4/seed-3.berkeley.o1test.net/tcp/10002/p2p/12D3KooWEiGVAFC7curXWXiGZyMWnZK9h8BKr88U8D5PKV3dXciv".parse().unwrap(),
    ];
    let listen_on = "/ip4/0.0.0.0/tcp/8302".parse().unwrap();
    let chain_id = b"667b328bfc09ced12191d099f234575b006b6b193f5441a6fa744feacd9744db";

    let mut swarm = mina_transport::swarm(local_key, chain_id, [listen_on], peers);
    while let Some(event) = swarm.next().await {
        match event {
            OutputEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            })) => {
                let _ = (propagation_source, message_id, message);
                // process new gossipsub message
                // swarm.behaviour_mut().publish(vec![]).unwrap();
            }
            OutputEvent::Behaviour(BehaviourEvent::Rpc(rpc::Event::ConnectionEstablished {
                peer_id,
                connection_id,
            })) => {
                // send heartbeat for each new peer
                swarm.behaviour_mut().rpc.send(
                    peer_id,
                    connection_id,
                    b"\x01\x00\x00\x00\x00\x00\x00\x00\x00".to_vec(),
                );
            }
            _ => {}
        }
    }
}
