use std::{env, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use tokio::fs;

type TTVChannel = String;
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Config {
    pub channel: Option<TTVChannel>,
    pub oauth: Option<String>,
    pub nick: Option<String>,
}

impl Config {
    pub async fn new() -> Self {
        let save_dir = Self::get_save_dir();

        match fs::read_to_string(&save_dir).await {
            Ok(c) => serde_json::from_str(&c).expect("Bad config"),
            Err(_) => Self {
                channel: None,
                oauth: None,
                nick: None,
            },
        }
    }

    fn get_save_dir() -> PathBuf {
        let mut save_dir = env::var("HOME").expect("Failed to get HOME");
        save_dir.push_str("/.ttvy_core/state.json");
        PathBuf::from_str(&save_dir).unwrap()
    }

    pub async fn load() -> Result<Self, tokio::io::Error> {
        let save_dir = Self::get_save_dir();

        match fs::read_to_string(&save_dir).await {
            Ok(c) => Ok(serde_json::from_str(&c).expect("Bad config")),
            Err(e) => Err(e),
        }
    }

    pub async fn save(&self) {
        let data = serde_json::json!(self).to_string();

        let save_dir = Self::get_save_dir();
        let _ = tokio::fs::create_dir_all(save_dir.parent().unwrap()).await;
        match tokio::fs::write(&save_dir, data).await {
            Ok(_) => println!("Saved config"),
            Err(_) => eprintln!("Failed to save config"),
        }
    }

    pub fn set_initial_channel(&mut self) {
        let args: Vec<String> = env::args().collect();

        if let Some(initial_channel) = args.get(1) {
            let _ = self.channel.insert(initial_channel.to_owned());
        }
    }

    pub async fn fetch_auth_token(&mut self) -> &mut Self {
        let token = http::get_ttv_token().await;
        let _ = self.oauth.insert(token);
        println!("Authtoken has been set!");
        self
    }
}

mod http {
    use std::{process::Stdio, sync::Arc};

    use axum::{
        http::{header, HeaderValue},
        response::IntoResponse,
        routing::{get, post},
        Extension, Json, Router,
    };

    use tokio::{
        fs::File,
        io::AsyncReadExt,
        process::Command,
        sync::mpsc::{channel, Receiver, Sender},
        task::JoinHandle,
    };

    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug)]
    struct TokenBody {
        pub token: String,
    }

    pub async fn get_ttv_token() -> String {
        let api_url: String = "https://id.twitch.tv/oauth2/authorize?\
            response_type=token\
            &client_id=m0y30jcckwn2a7m7hh0djrg47wvbuk\
            &scope=chat%3Aread%20chat%3Aedit\
            &redirect_uri=http://localhost:4537"
            .to_string();

        println!("Complete authentication at\n{}", &api_url);
        if open_browser(&api_url).await.is_err() {
            println!("Failed to open browser automatically, please navigate manually.")
        }

        println!("Waiting for token...");
        let (token_tx, mut token_rx) = channel::<String>(1);
        let (shutdown_tx, shutdown_rx) = channel::<()>(1);

        let _handle = start_webserver(token_tx, shutdown_rx);

        let msg = token_rx.recv().await.unwrap();
        shutdown_tx.send(()).await.unwrap();
        msg
    }

    async fn open_browser(url: &str) -> Result<std::process::ExitStatus, std::io::Error> {
        Command::new("open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
    }

    fn start_webserver(token_tx: Sender<String>, mut shutdown_rx: Receiver<()>) -> JoinHandle<()> {
        let state = Arc::new(token_tx);
        tokio::spawn(async move {
            // build our application with a single route
            let app = Router::new()
                .route("/token", post(handle_token_route))
                .route("/", get(serve_index))
                .route("/script.js", get(serve_script))
                .layer(Extension(state));

            let listener = tokio::net::TcpListener::bind("0.0.0.0:4537").await.unwrap();

            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown_rx.recv().await;
                })
                .await
                .unwrap()
        })
    }

    async fn serve_static_file(path: &str) -> String {
        let mut file = File::open(path).await.unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).await.unwrap();
        contents
    }

    async fn serve_index() -> impl IntoResponse {
        let mut res = serve_static_file("web/index.html").await.into_response();

        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str("text/html").unwrap(),
        );

        res
    }

    async fn serve_script() -> impl IntoResponse {
        let mut res = serve_static_file("web/script.js").await.into_response();

        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str("application/javascript").unwrap(),
        );

        res
    }

    async fn handle_token_route(
        state: Extension<Arc<Sender<String>>>,
        Json(payload): Json<TokenBody>,
    ) {
        state.0.send(payload.token).await.unwrap();
        "OK".to_string();
    }
}
