//! Simple in-memory key/value store showing features of axum.
//!
//! Run with:
//!
//! ```not_rust
//! cargo run -p example-key-value-store
//! ```

use axum::{
    body::Bytes,
    error_handling::HandleErrorLayer,
    extract::{DefaultBodyLimit, Path, State},
    handler::Handler,
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
    Router,
};
use hyper::service::make_service_fn;
use rand::RngCore;
use std::{
    borrow::Cow,
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};
use tower::{BoxError, ServiceBuilder};

use tower_http::{
    compression::CompressionLayer, limit::RequestBodyLimitLayer, trace::TraceLayer,
    validate_request::ValidateRequestHeaderLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};


#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "example_key_value_store=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let shared_state = SharedState::default();

    let seed_node = std::env::var("seed_node").expect("failed to parse seed node value from env");
    let port = std::env::var("port").expect("failed to parse port value from env");

    let is_seed_node = seed_node.parse::<bool>().expect("cannot parse boolean");

    if is_seed_node {
        println!("This is SEED NODE!!!!!!!");
    } else {

        print!(" THIS IS NOT A SEED NODE !!!!!!!")
    }
    // Build our application by composing routes
    let app = Router::new()
        .route(
            "/:key",
            // Add compression to `kv_get`
            get(kv_get.layer(CompressionLayer::new()))
                // But don't compress `kv_set`
                .post_service(
                    kv_set
                        .layer((
                            DefaultBodyLimit::disable(),
                            RequestBodyLimitLayer::new(1024 * 5_000 /* ~5mb */),
                        ))
                        .with_state(Arc::clone(&shared_state)),
                ),
        )
        .route("/keys", get(list_keys))
        // Nest our admin routes under `/admin`
        .nest("/admin", admin_routes())
        // Add middleware to all routes
        .layer(
            ServiceBuilder::new()
                // Handle errors from middleware
                .layer(HandleErrorLayer::new(handle_error))
                .load_shed()
                .concurrency_limit(1024)
                .timeout(Duration::from_secs(10))
                .layer(TraceLayer::new_for_http()),
        )
        .with_state(Arc::clone(&shared_state));

    let mut partial_add = "[::]:".to_string();
    partial_add.push_str(&port);

    let partial_add = partial_add.parse().unwrap();
    dbg!(partial_add);
    
    let h = tokio::task::spawn(async move {
        axum::Server::bind(&partial_add)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });

    println!("Done moving server !!!");
    
    if !is_seed_node {
        let client = reqwest::Client::builder()
                        .no_proxy()
                        .build()
                        .unwrap();
        println!("Creating Client......");

        let port = std::env::var("seed_port").expect("failed to parse port value from env");
        // For now hardcoding the container url.
        let mut path =  "seed-node:".to_string();
        let port  = port.to_string();
        path.push_str(port.to_string().as_str());
        println!("Seed Path : {:?}", &path);
        let mut rand = rand::thread_rng();
        let get_url = format!("http://{}/keys", &path);
        let post_url = format!("http://{}/{}", &path, rand.next_u64());

        for _ in 1..4 {

            println!("Sending Request ");
            let r = client
                                .post(&post_url)
                                .body("abbbb")
                                .send()
                                .await
                                .expect("did not recieve any values");
            println!("Response : {:?}", r);
        }

        let r = client.get(&get_url).send().await.expect("failed on get");

        println!("Response: {:?}", r);

    }

    h.await.expect("done waiting ................");
}

type SharedState = Arc<RwLock<AppState>>;

#[derive(Default)]
struct AppState {
    db: HashMap<String, Bytes>,
}

async fn kv_get(
    Path(key): Path<String>,
    State(state): State<SharedState>,
) -> Result<Bytes, StatusCode> {
    let db = &state.read().unwrap().db;

    if let Some(value) = db.get(&key) {
        Ok(value.clone())
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn kv_set(Path(key): Path<String>, State(state): State<SharedState>, bytes: Bytes) {
    state.write().unwrap().db.insert(key, bytes);
}

async fn list_keys(State(state): State<SharedState>) -> String {
    let db = &state.read().unwrap().db;

    db.keys()
        .map(|key| key.to_string())
        .collect::<Vec<String>>()
        .join("\n")
}

fn admin_routes() -> Router<SharedState> {
    async fn delete_all_keys(State(state): State<SharedState>) {
        state.write().unwrap().db.clear();
    }

    async fn remove_key(Path(key): Path<String>, State(state): State<SharedState>) {
        state.write().unwrap().db.remove(&key);
    }

    Router::new()
        .route("/keys", delete(delete_all_keys))
        .route("/key/:key", delete(remove_key))
        // Require bearer auth for all admin routes
        .layer(ValidateRequestHeaderLayer::bearer("secret-token"))
}

async fn handle_error(error: BoxError) -> impl IntoResponse {
    if error.is::<tower::timeout::error::Elapsed>() {
        return (StatusCode::REQUEST_TIMEOUT, Cow::from("request timed out"));
    }

    if error.is::<tower::load_shed::error::Overloaded>() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Cow::from("service is overloaded, try again later"),
        );
    }

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Cow::from(format!("Unhandled internal error: {}", error)),
    )
}
