use tokio::sync::mpsc::{Receiver, Sender};

use fast_websocket_client as ws;

use super::chat_controller::{ConnectConfig, Controller};
pub use super::config::Config;

#[derive(Debug)]
pub struct Chat {
    controller: Controller,
    output: Receiver<ChatMessage>,
    pub config: Config,
}

#[derive(Debug)]
pub struct ChatMessage {
    pub author: String,
    pub color: Option<String>,
    pub message: String,
}

impl Chat {
    pub fn new() -> Self {
        let mut controller = Controller::new();
        let output = controller.take_receiver().unwrap();
        let config = Config::default();

        Self {
            controller,
            output,
            config,
        }
    }

    pub async fn init(&mut self) -> &mut Self {
        if let Ok(config) = Config::load().await {
            println!("Loaded config");
            self.config = config;
        }
        self
    }

    pub async fn send(&self, chat_message: String) {
        self.controller.send(chat_message).await;
    }

    pub async fn receive(&mut self) -> ChatMessage {
        loop {
            match self.output.recv().await {
                Some(msg) => return msg,
                None => {
                    eprintln!("Encountered empty message");
                }
            }
        }
    }

    pub fn join(&mut self, channel: &str) {
        self.config.channel.replace(channel.to_string());
        self.controller.join(self.config.clone().into());
    }

    pub fn leave(&mut self) {
        self.controller.leave();
    }

    pub fn reconnect(&mut self) {
        if self.config.channel.is_some() {
            self.controller.join(self.config.clone().into());
        } else {
            println!("No recently joined channel to reconnect to");
        }
    }

    pub async fn fetch_auth_token(&mut self) -> &mut Self {
        Config::fetch_auth_token(&mut self.config).await;
        self
    }
}

///
/// `incoming_message_tx` is a sender of messages. The websocket will transmit its incoming
/// messages over this Sender.
/// `outgoing_message_rx` is a receiver of messages transmitted by users of this library
///
pub(super) async fn connect(
    connect_config: ConnectConfig,
    incoming_message_tx: Sender<ChatMessage>,
    mut outgoing_message_rx: Receiver<String>,
) {
    {
        let ConnectConfig {
            channel,
            mut oauth,
            mut nick,
        } = connect_config;

        let channel = channel.unwrap();

        let join = format!("JOIN #{}\n\r", &channel);
        let oauth = format!(
            "PASS oauth:{}",
            oauth.get_or_insert_with(|| "blah".to_string())
        );
        let nick = format!(
            "NICK {}\n\r",
            nick.get_or_insert_with(|| "justinfan354678".to_string())
        );

        let mut conn = ws::connect("ws://irc-ws.chat.twitch.tv:80").await.unwrap();
        conn.set_auto_pong(true);

        conn.send_string(&oauth).await.unwrap();
        conn.send_string(&nick).await.unwrap();
        conn.send_string(&join).await.unwrap();
        conn.send_string("CAP REQ :twitch.tv/tags").await.unwrap();

        let mut read_tags_allowed = false;
        let mut last_sent_message = String::new();
        println!("Joined channel #{}", &channel);
        loop {
            tokio::select! {
                res = conn.receive_frame() => {
                    match res {
                        Ok(f) => {
                            let msg = if let Ok(s) = std::str::from_utf8(&f.payload) {
                                s.to_string()
                            } else {
                                f.payload
                                    .iter()
                                    .map(|v| -> char { (*v).into() })
                                    .collect::<String>()
                            };

                            handle_websocket_message(&incoming_message_tx, msg, &mut read_tags_allowed).await;
                        }
                        Err(e) => {
                            println!("{}", e);
                            break;
                        }
                    }
                }
                msg = outgoing_message_rx.recv() => {
                    if let Some(mut msg) = msg {
                        if msg.is_empty() {
                            msg = last_sent_message.clone();
                        }

                        if last_sent_message == msg {
                            if msg.contains(" \u{E0000}") {
                                msg = msg.strip_suffix(" \u{E0000}").unwrap().to_string();
                            } else {
                                msg.push_str(" \u{E0000}");
                            }
                        }

                        last_sent_message = msg.clone();

                        let fmt = format!("PRIVMSG #{} :{}", &channel, &msg);
                        let _ = conn.send_string(&fmt).await;
                    }
                }
            };
        }
    }
}

async fn handle_websocket_message(
    incoming_message_tx: &Sender<ChatMessage>,
    msg: String,
    read_tags_allowed: &mut bool,
) {
    match msg {
        m if m.contains("ACK :twitch.tv/tags") => {
            *read_tags_allowed = true;
        }
        m if *read_tags_allowed && m.contains("PRIVMSG") => {
            if let Some(user_message) = parse::format_user_message_with_tags(&m) {
                incoming_message_tx
                    .send(user_message)
                    .await
                    .expect("Controller proxy should be set up")
            }
        }
        m if m.contains("PRIVMSG") => {
            if let Some(user_message) = parse::format_user_message(&m) {
                incoming_message_tx
                    .send(user_message)
                    .await
                    .expect("Controller proxy should be set up");
            }
        }
        m => {
            println!("{}", &m);
        }
    }
}

mod parse {
    use std::collections::HashMap;

    use super::ChatMessage;

    pub fn format_user_message(str: &str) -> Option<ChatMessage> {
        let str = str.split_once("\r\n").unwrap().0;

        let author = if let Some((author, _)) = str.split_once('!') {
            Some(author.get(1..).unwrap().to_string())
        } else {
            None
        };

        let message = str.splitn(3, ':').last().unwrap().to_string();

        if let (Some(author), message) = (author, message) {
            Some(ChatMessage {
                author,
                color: None,
                message,
            })
        } else {
            None
        }
    }

    pub fn format_user_message_with_tags(str: &str) -> Option<ChatMessage> {
        let str = str.split_once("\r\n").unwrap().0;

        let (tags, _author_info, message) = {
            let (tags, tail) = match str.split_once(" :") {
                Some((tags, tail)) => (tags, tail),
                None => return None,
            };

            let (author_info, message) = match tail.split_once(" :") {
                Some((author_info, message)) => (author_info, message),
                None => return None,
            };
            (tags, author_info, message)
        };

        let tags = parse_tags(tags);

        let author = match tags.get("display-name").as_mut() {
            Some(author) => author.to_string(),
            None => return None,
        };

        let color = tags.get("color").as_mut().map(|color| color.to_string());

        Some(ChatMessage {
            author,
            color,
            message: message.to_owned(),
        })
    }

    fn parse_tags(tags: &str) -> HashMap<&str, &str> {
        tags.split(';')
            .filter_map(|pair| pair.split_once('='))
            .collect()
    }
}
