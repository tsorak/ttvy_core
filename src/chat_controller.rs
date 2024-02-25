use std::sync::Arc;

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::chat::{connect, ChatMessage};
use super::config::Config;

#[derive(Debug, Clone, Default)]
pub struct ConnectConfig {
    pub channel: Option<String>,
    pub oauth: Option<String>,
    pub nick: Option<String>,
}

impl From<Config> for ConnectConfig {
    fn from(value: Config) -> Self {
        let Config {
            channel,
            oauth,
            nick,
            ..
        } = value;

        Self {
            channel,
            oauth,
            nick,
        }
    }
}

#[derive(Debug)]
pub struct Controller {
    proxy_tx: Sender<ChatMessage>,
    proxy_rx: Option<Receiver<ChatMessage>>,
    websocket_tx: Arc<Mutex<Option<Sender<String>>>>,
    handle: Option<JoinHandle<()>>,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub fn new() -> Self {
        let (tx, rx) = channel::<ChatMessage>(128);

        Self {
            proxy_tx: tx,
            proxy_rx: Some(rx),
            websocket_tx: Arc::new(Mutex::new(None)),
            handle: None,
        }
    }

    pub async fn send(&self, chat_message: String) {
        let lock = self.websocket_tx.lock().await;
        if let Some(tx) = lock.as_ref() {
            let _ = tx.send(chat_message).await;
        }
    }

    ///
    /// Can only be called once, eg only the first call returns `Some`.
    ///
    pub fn take_receiver(&mut self) -> Option<Receiver<ChatMessage>> {
        self.proxy_rx.take()
    }

    pub fn join(&mut self, connect_config: ConnectConfig) {
        if self.handle.is_none() {
            self.supervise(connect_config);
        } else {
            let handle = self.handle.as_ref().unwrap();
            handle.abort();

            self.supervise(connect_config);
        }
    }

    pub fn leave(&mut self) -> &mut Self {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }

        self
    }

    fn supervise(&mut self, connect_config: ConnectConfig) -> &mut Self {
        let controller_websocket_tx = self.websocket_tx.clone();
        let proxy_tx = self.proxy_tx.clone();

        let handle = tokio::spawn(async move {
            loop {
                let connect_config = connect_config.clone();
                //setup proxy channel for receiving messages from websocket
                // ttvy_core <-- websocket <-- (twitch server)
                let (incoming_tx, incoming_rx) = channel::<ChatMessage>(128);

                //setup channel for sending messages over websocket
                // ttvy_core --> websocket --> (twitch server)
                let (websocket_tx, outgoing_rx) = channel::<String>(128);
                let mut controller_websocket_tx = controller_websocket_tx.lock().await;
                *controller_websocket_tx = Some(websocket_tx);

                let proxy = spawn_proxy_worker(incoming_rx, &proxy_tx);
                let _result =
                    tokio::spawn(
                        async move { connect(connect_config, incoming_tx, outgoing_rx).await },
                    )
                    .await;
                proxy.abort();
            }
        });

        self.handle = Some(handle);
        self
    }
}

fn spawn_proxy_worker(mut rx: Receiver<ChatMessage>, tx: &Sender<ChatMessage>) -> JoinHandle<()> {
    let tx = tx.clone();

    tokio::spawn(async move {
        loop {
            if let Some(msg) = rx.recv().await {
                let _result = tx.send(msg).await;
            }
        }
    })
}
