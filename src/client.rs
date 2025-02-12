use axum::{
    body::Body,
    extract::{Path, State},
    http::{header::CONTENT_TYPE, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use movie::movie_service_client::MovieServiceClient;
use movie::{CreateMovieRequest, DeleteMovieRequest, ReadMovieRequest, UpdateMovieRequest};
use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use prometheus_client_derive_encode::{EncodeLabelSet, EncodeLabelValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::Request;
use uuid::Uuid;

use opentelemetry::{
    global,
    propagation::Injector,
    trace::{SpanKind, TraceContextExt, TraceError, Tracer},
    Context, KeyValue,
};
use opentelemetry_sdk::{ trace::SdkTracerProvider, Resource};


pub mod movie {
    tonic::include_proto!("movie");
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct MethodLabels {
    pub method: Method,
}

#[derive(Debug, Clone)]
pub struct Metrics {
    requests: Family<MethodLabels, Counter>,
}

impl Metrics {
    pub fn inc_requests(&self, method: Method) {
        self.requests.get_or_create(&MethodLabels { method }).inc();
    }
}

#[derive(Debug)]
pub struct AppState {
    pub registry: Registry,
    pub grpc_client: Arc<tokio::sync::Mutex<MovieServiceClient<tonic::transport::Channel>>>,
    pub metrics: Arc<Mutex<Metrics>>,
}

fn init_tracer_provider() -> Result<opentelemetry_sdk::trace::SdkTracerProvider, TraceError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
    
        .with_tonic()
        .build()?;

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name("movie-client")
                .build(),
        )
        .build())
}

// Metadata map for trace context injection
struct MetadataMap<'a>(&'a mut tonic::metadata::MetadataMap);

impl<'a> Injector for MetadataMap<'a> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes()) {
            if let Ok(val) = tonic::metadata::MetadataValue::try_from(&value) {
                self.0.insert(key, val);
            }
        }
    }
}

pub async fn metrics_handler(State(state): State<Arc<Mutex<AppState>>>) -> impl IntoResponse {
    let state = state.lock().await;
    let mut buffer = String::new();
    encode(&mut buffer, &state.registry).unwrap();

    Response::builder()
        .status(StatusCode::OK)
        .header(
            CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )
        .body(Body::from(buffer))
        .unwrap()
}

// Existing input and response structs
#[derive(Serialize, Deserialize)]
struct MovieInput {
    id: Option<String>,
    title: String,
    genre: String,
}

#[derive(Serialize, Deserialize)]
struct MovieResponse {
    id: String,
    title: String,
    genre: String,
}

// Error response helper function
fn error_response(
    code: axum::http::StatusCode,
    message: &str,
) -> (axum::http::StatusCode, Json<Value>) {
    (code, Json(json!({ "error": message })))
}

// CRUD handlers with metrics tracking
async fn create_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Json(input): Json<MovieInput>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    // Increment metrics
    let metrics = state.lock().await.metrics.clone();
    metrics.lock().await.inc_requests(Method::Post);

    let tracer = global::tracer("movie-client");
    let span = tracer
        .span_builder("CreateMovie")
        .with_kind(SpanKind::Client)
        .with_attributes([
            KeyValue::new("component", "grpc"),
            KeyValue::new("movie.title", input.title.clone()),
            KeyValue::new("movie.genre", input.genre.clone()),
        ])
        .start(&tracer);
    let cx = Context::current_with_span(span);

    let state = state.lock().await;
    let mut client = state.grpc_client.lock().await;

    let movie_id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut request = Request::new(CreateMovieRequest {
        movie: Some(movie::Movie {
            id: movie_id.clone(),
            title: input.title,
            genre: input.genre,
        }),
    });

    // Inject tracing context
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MetadataMap(request.metadata_mut()))
    });

    let response_result = client.create_movie(request).await;

    let status = match &response_result {
        Ok(_) => "OK".to_string(),
        Err(status) => status.code().to_string(),
    };

    cx.span().add_event(
        "Create movie request completed",
        vec![KeyValue::new("status", status.clone())],
    );

    match response_result {
        Ok(response) => {
            let movie = response.into_inner().movie.unwrap();
            Ok(Json(json!({
                "id": movie.id,
                "title": movie.title,
                "genre": movie.genre
            })))
        }
        Err(status) => Err(error_response(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

async fn get_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    // Increment metrics
    let metrics = state.lock().await.metrics.clone();
    metrics.lock().await.inc_requests(Method::Get);

    let tracer = global::tracer("movie-client");
    let span = tracer
        .span_builder("GetMovie")
        .with_kind(SpanKind::Client)
        .with_attributes([
            KeyValue::new("component", "grpc"),
            KeyValue::new("movie.id", id.clone()),
        ])
        .start(&tracer);
    let cx = Context::current_with_span(span);

    let state = state.lock().await;
    let mut client = state.grpc_client.lock().await;

    let mut request = Request::new(ReadMovieRequest { id });

    // Inject tracing context
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MetadataMap(request.metadata_mut()))
    });

    let response_result = client.get_movie(request).await;

    let status = match &response_result {
        Ok(_) => "OK".to_string(),
        Err(status) => status.code().to_string(),
    };

    cx.span().add_event(
        "Get movie request completed",
        vec![KeyValue::new("status", status.clone())],
    );

    match response_result {
        Ok(response) => {
            let movie = response.into_inner().movie.unwrap();
            Ok(Json(json!({
                "id": movie.id,
                "title": movie.title,
                "genre": movie.genre
            })))
        }
        Err(status) => Err(error_response(
            axum::http::StatusCode::NOT_FOUND,
            &status.to_string(),
        )),
    }
}

async fn list_movies(
    State(state): State<Arc<Mutex<AppState>>>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    // Increment metrics
    let metrics = state.lock().await.metrics.clone();
    metrics.lock().await.inc_requests(Method::Get);

    let tracer = global::tracer("movie-client");
    let span = tracer
        .span_builder("ListMovies")
        .with_kind(SpanKind::Client)
        .with_attributes([KeyValue::new("component", "grpc")])
        .start(&tracer);
    let cx = Context::current_with_span(span);

    let state = state.lock().await;
    let mut client = state.grpc_client.lock().await;
    let mut request = Request::new(movie::ReadMoviesRequest {});

    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MetadataMap(request.metadata_mut()))
    });

    let response_result = client.get_movies(request).await;

    let _status = match &response_result {
        Ok(response) => {
            cx.span().add_event(
                "List movies request completed",
                vec![
                    KeyValue::new("status", "OK"),
                    KeyValue::new("movie_count", response.get_ref().movies.len() as i64),
                ],
            );
            "OK".to_string()
        }
        Err(status) => status.code().to_string(),
    };

    match response_result {
        Ok(response) => {
            let movies: Vec<movie::Movie> = response.into_inner().movies;
            let movie_responses: Vec<MovieResponse> = movies
                .into_iter()
                .map(|movie| MovieResponse {
                    id: movie.id,
                    title: movie.title,
                    genre: movie.genre,
                })
                .collect();
            Ok(Json(serde_json::to_value(movie_responses).unwrap()))
        }
        Err(status) => Err(error_response(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

async fn update_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(id): Path<String>,
    Json(input): Json<MovieInput>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    // Increment metrics
    let metrics = state.lock().await.metrics.clone();
    metrics.lock().await.inc_requests(Method::Put);

    let tracer = global::tracer("movie-client");
    let span = tracer
        .span_builder("UpdateMovie")
        .with_kind(SpanKind::Client)
        .with_attributes([
            KeyValue::new("component", "grpc"),
            KeyValue::new("movie.id", id.clone()),
            KeyValue::new("movie.title", input.title.clone()),
            KeyValue::new("movie.genre", input.genre.clone()),
        ])
        .start(&tracer);
    let cx = Context::current_with_span(span);

    let state = state.lock().await;
    let mut client = state.grpc_client.lock().await;

    let mut request = Request::new(UpdateMovieRequest {
        movie: Some(movie::Movie {
            id,
            title: input.title,
            genre: input.genre,
        }),
    });

    // Inject tracing context
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MetadataMap(request.metadata_mut()))
    });

    let response_result = client.update_movie(request).await;

    let status = match &response_result {
        Ok(_) => "OK".to_string(),
        Err(status) => status.code().to_string(),
    };

    cx.span().add_event(
        "Update movie request completed",
        vec![KeyValue::new("status", status.clone())],
    );

    match response_result {
        Ok(response) => {
            let movie = response.into_inner().movie.unwrap();
            Ok(Json(json!({
                "id": movie.id,
                "title": movie.title,
                "genre": movie.genre
            })))
        }
        Err(status) => Err(error_response(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

async fn delete_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    let metrics = state.lock().await.metrics.clone();
    metrics.lock().await.inc_requests(Method::Delete);

    let tracer = global::tracer("movie-client");
    let span = tracer
        .span_builder("DeleteMovie")
        .with_kind(SpanKind::Client)
        .with_attributes([
            KeyValue::new("component", "grpc"),
            KeyValue::new("movie.id", id.clone()),
        ])
        .start(&tracer);
    let cx = Context::current_with_span(span);

    let state = state.lock().await;
    let mut client = state.grpc_client.lock().await;

    let mut request = Request::new(DeleteMovieRequest { id });

    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MetadataMap(request.metadata_mut()))
    });

    let response_result = client.delete_movie(request).await;

    let status = match &response_result {
        Ok(_) => "OK".to_string(),
        Err(status) => status.code().to_string(),
    };

    cx.span().add_event(
        "Delete movie request completed",
        vec![KeyValue::new("status", status.clone())],
    );

    match response_result {
        Ok(response) => Ok(Json(json!({ "success": response.into_inner().success }))),
        Err(status) => Err(error_response(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tracer_provider = init_tracer_provider().expect("Hello");

    global::set_tracer_provider(tracer_provider.clone());

    let metrics = Metrics {
        requests: Family::default(),
    };

    let mut registry = Registry::default();
    registry.register(
        "movie_requests",
        "Total number of movie service requests",
        metrics.requests.clone(),
    );

    let grpc_client = MovieServiceClient::connect("http://[::1]:50051").await?;

    let state = Arc::new(Mutex::new(AppState {
        registry,
        grpc_client: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        metrics: Arc::new(Mutex::new(metrics)),
    }));


    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/movies", get(list_movies).post(create_movie))
        .route(
            "/movies/{id}",
            get(get_movie).put(update_movie).delete(delete_movie),
        )
        .with_state(state);

    // Bind and serve
    let listener = tokio::net::TcpListener::bind("127.0.0.1:5000").await?;
    println!("Server running on http://127.0.0.1:5000");

    axum::serve(listener, app).await?;

    Ok(())
}