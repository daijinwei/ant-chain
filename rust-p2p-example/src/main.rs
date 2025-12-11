use std::env;
use std::error::Error;
use std::time::Duration;

use libp2p::floodsub::Floodsub;
use libp2p::{mdns, noise, tcp, yamux, Swarm};
use log::{error, info};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

use crate::behaviour::RecipeBehaviour;
use crate::consts::{KEYS, PEER_ID, TOPIC};
use crate::handlers::{
    handle_create_recipe, handle_list_peers, handle_list_recipes, handle_publish_recipe,
    handle_swarm_event,
};
use crate::models::EventType;

mod behaviour;
mod consts;
mod handlers;
mod models;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env::set_var("RUST_LOG", "info");
    pretty_env_logger::init();

    info!("Peer Id: {}", PEER_ID.clone());
    // 创建一个无限容量的队列， 返回发送器，接收器
    let (response_sender, mut response_rcv) = mpsc::unbounded_channel();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(KEYS.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_key| RecipeBehaviour {
            flood_sub: Floodsub::new(*PEER_ID),
            mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), KEYS.public().to_peer_id())
                .expect("can create mdns"),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(5)))
        .build();
    // 启动监听
    Swarm::listen_on(
        &mut swarm,
        "/ip4/0.0.0.0/tcp/0"
            .parse()
            .expect("can get a local socket"),
    )
    .expect("swarm can be started");

    swarm.behaviour_mut().flood_sub.subscribe(TOPIC.clone());

    // 创建异步输入标准输入是在 Tokio 异步运行时 中创建一个 异步读取标准输入（stdin）的流。我详细拆解一下。
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    loop {
        // 1. 异步监听用户输入（stdin）
        // 2. 异步监听来自其他节点的响应（response channel）
        // 3. 异步处理 libp2p Swarm 网络事件（连接、消息等）
        let evt: Option<EventType> = {
            tokio::select! {
                line = stdin.next_line() => Some(EventType::Input(line.expect("can get line").expect("can read line from stdin"))),
                response = response_rcv.recv() => Some(EventType::Response(response.expect("response exists"))),
                _ = handle_swarm_event(response_sender.clone(), &mut swarm) => None,
            }
        };
        // 根据事件类型执行不同逻辑（发布消息、处理命令）
        if let Some(event) = evt {
            match event {
                EventType::Response(resp) => {
                    let json = serde_json::to_string(&resp).expect("can jsonify response");
                    swarm
                        .behaviour_mut()
                        .flood_sub
                        .publish(TOPIC.clone(), json.as_bytes());
                }
                EventType::Input(line) => match line.as_str() {
                    "ls p" => handle_list_peers(&mut swarm).await,
                    cmd if cmd.starts_with("create r") => handle_create_recipe(cmd).await,
                    cmd if cmd.starts_with("publish r") => handle_publish_recipe(cmd).await,
                    cmd if cmd.starts_with("ls r") => handle_list_recipes(cmd, &mut swarm).await,
                    _ => error!("unknown command: {:?}", line),
                },
            }
        }
    }
}