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
use opentelemetry_otlp::WithExportConfig;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use prometheus_client::{encoding::text::encode, metrics::gauge::Gauge};
use prometheus_client_derive_encode::{EncodeLabelSet, EncodeLabelValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use sysinfo::System;
use tokio::sync::Mutex;
use tonic::transport::Channel;
use tonic::{Request, Status};
use uuid::Uuid;

use opentelemetry::{
    global::{self, BoxedTracer},
    propagation::Injector,
    trace::{SpanKind, TraceContextExt, Tracer},
    Context, KeyValue,
};
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace as sdktrace};

use opentelemetry_sdk::Resource;

pub mod movie {
    tonic::include_proto!("movie");
}

fn get_thread_count(pid: usize) -> Option<i64> {
    let path = format!("/proc/{}/status", pid);
    if let Ok(contents) = fs::read_to_string(path) {
        for line in contents.lines() {
            if line.starts_with("Threads:") {
                if let Some(thread_count) = line.split_whitespace().nth(1) {
                    return thread_count.parse::<i64>().ok();
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct SystemMetrics {
    pub memory_alloc_bytes: Gauge,
    pub memory_sys_bytes: Gauge,
    pub available_memory: Counter,
    pub thread_usage: Gauge,
    pub total_cpu_usage: Counter,
    pub process_start_time: Gauge,
}

impl SystemMetrics {
    pub fn new() -> Self {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let metrics = Self {
            memory_alloc_bytes: Gauge::default(),
            memory_sys_bytes: Gauge::default(),
            available_memory: Counter::default(),
            thread_usage: Gauge::default(),
            total_cpu_usage: Counter::default(),
            process_start_time: Gauge::default(),
        };

        metrics.process_start_time.set(start_time as i64);
        metrics
    }

    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "process_memory_alloc_bytes",
            "Current memory allocation in bytes",
            self.memory_alloc_bytes.clone(),
        );

        registry.register(
            "process_memory_sys_bytes",
            "Total system memory in bytes",
            self.memory_sys_bytes.clone(),
        );

        registry.register(
            "process_memory_frees_total",
            "Total Available Memory",
            self.available_memory.clone(),
        );

        registry.register(
            "process_thread_total",
            "Thread total",
            self.thread_usage.clone(),
        );

        registry.register(
            "total_cpu_usage",
            "Total cpu usage",
            self.total_cpu_usage.clone(),
        );

        registry.register(
            "process_start_time_seconds",
            "Start time of the process since unix epoch in seconds",
            self.process_start_time.clone(),
        );
    }

    pub async fn update_metrics(&self) {
        let mut sys = System::new_all();
        sys.refresh_all();

        let pid = std::process::id() as usize;

        if let Some(process) = sys.process(sysinfo::Pid::from(pid)) {
            let current_memory = process.memory() as i64;
            self.memory_alloc_bytes.set(current_memory);
            self.memory_sys_bytes.set(process.virtual_memory() as i64);

            let available_memory = sys.available_memory() / 1_024;
            self.available_memory.inc_by(available_memory);

            let total_cpu_usage = sys.global_cpu_usage();
            self.total_cpu_usage.inc_by(total_cpu_usage as u64);

            if let Some(thread_count) = get_thread_count(pid) {
                self.thread_usage.set(thread_count);
            }
        }
    }
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
    pub movie_service: Arc<MovieService>,
    pub grpc_client: Arc<tokio::sync::Mutex<MovieServiceClient<tonic::transport::Channel>>>,
    pub metrics: Arc<Mutex<Metrics>>,
    pub system_metrics: Arc<SystemMetrics>,
}

fn init_tracer() -> opentelemetry_sdk::trace::SdkTracerProvider {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint("http://otel-collector:4317")
        .build()
        .expect("Failed to build OTLP exporter");

    let provider = sdktrace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name("movie-client")
                .build(),
        )
        .build();

    global::set_tracer_provider(provider.clone());
    provider
}

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

pub struct MovieService {
    grpc_client: Arc<Mutex<MovieServiceClient<Channel>>>,
    metrics: Arc<Mutex<Metrics>>,
}

impl MovieService {
    pub fn new(
        grpc_client: Arc<Mutex<MovieServiceClient<Channel>>>,
        metrics: Arc<Mutex<Metrics>>,
    ) -> Self {
        Self {
            grpc_client,
            metrics,
        }
    }

    fn get_tracer(&self) -> BoxedTracer {
        global::tracer("movie-client")
    }

    pub async fn create_movie(&self, input: MovieInput) -> Result<MovieResponse, Status> {
        self.metrics.lock().await.inc_requests(Method::Post);

        let tracer = self.get_tracer();
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

        let movie_id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut client = self.grpc_client.lock().await;

        let mut request = Request::new(CreateMovieRequest {
            movie: Some(movie::Movie {
                id: movie_id.clone(),
                title: input.title,
                genre: input.genre,
            }),
        });

        self.inject_trace_context(&cx, &mut request);

        let response_result = client.create_movie(request).await;
        self.add_completion_event(
            &cx,
            &response_result,
            "Create movie request completed".to_string(),
        );

        match response_result {
            Ok(response) => {
                let movie = response.into_inner().movie.unwrap();
                Ok(MovieResponse {
                    id: movie.id,
                    title: movie.title,
                    genre: movie.genre,
                })
            }
            Err(status) => Err(status),
        }
    }

    pub async fn get_movie(&self, id: String) -> Result<MovieResponse, Status> {
        self.metrics.lock().await.inc_requests(Method::Get);

        let tracer = self.get_tracer();
        let span = tracer
            .span_builder("GetMovie")
            .with_kind(SpanKind::Client)
            .with_attributes([
                KeyValue::new("component", "grpc"),
                KeyValue::new("movie.id", id.clone()),
            ])
            .start(&tracer);
        let cx = Context::current_with_span(span);

        let mut client = self.grpc_client.lock().await;
        let mut request = Request::new(ReadMovieRequest { id });

        self.inject_trace_context(&cx, &mut request);

        let response_result = client.get_movie(request).await;
        self.add_completion_event(
            &cx,
            &response_result,
            "Get movie request completed".to_string(),
        );

        match response_result {
            Ok(response) => {
                let movie = response.into_inner().movie.unwrap();
                Ok(MovieResponse {
                    id: movie.id,
                    title: movie.title,
                    genre: movie.genre,
                })
            }
            Err(status) => Err(status),
        }
    }

    pub async fn list_movies(&self) -> Result<Vec<MovieResponse>, Status> {
        self.metrics.lock().await.inc_requests(Method::Get);

        let tracer = self.get_tracer();
        let span = tracer
            .span_builder("ListMovies")
            .with_kind(SpanKind::Client)
            .with_attributes([KeyValue::new("component", "grpc")])
            .start(&tracer);
        let cx = Context::current_with_span(span);

        let mut client = self.grpc_client.lock().await;
        let mut request = Request::new(movie::ReadMoviesRequest {});

        self.inject_trace_context(&cx, &mut request);

        let response_result = client.get_movies(request).await;

        match &response_result {
            Ok(response) => {
                cx.span().add_event(
                    "List movies request completed",
                    vec![
                        KeyValue::new("status", "OK"),
                        KeyValue::new("movie_count", response.get_ref().movies.len() as i64),
                    ],
                );
            }
            Err(status) => {
                cx.span().add_event(
                    "List movies request completed",
                    vec![KeyValue::new("status", status.code().to_string())],
                );
            }
        }

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
                Ok(movie_responses)
            }
            Err(status) => Err(status),
        }
    }

    pub async fn update_movie(
        &self,
        id: String,
        input: MovieInput,
    ) -> Result<MovieResponse, Status> {
        self.metrics.lock().await.inc_requests(Method::Put);

        let tracer = self.get_tracer();
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

        let mut client = self.grpc_client.lock().await;
        let mut request = Request::new(UpdateMovieRequest {
            movie: Some(movie::Movie {
                id,
                title: input.title,
                genre: input.genre,
            }),
        });

        self.inject_trace_context(&cx, &mut request);

        let response_result = client.update_movie(request).await;
        self.add_completion_event(
            &cx,
            &response_result,
            "Update movie request completed".to_string(),
        );

        match response_result {
            Ok(response) => {
                let movie = response.into_inner().movie.unwrap();
                Ok(MovieResponse {
                    id: movie.id,
                    title: movie.title,
                    genre: movie.genre,
                })
            }
            Err(status) => Err(status),
        }
    }

    pub async fn delete_movie(&self, id: String) -> Result<bool, Status> {
        self.metrics.lock().await.inc_requests(Method::Delete);

        let tracer = self.get_tracer();
        let span = tracer
            .span_builder("DeleteMovie")
            .with_kind(SpanKind::Client)
            .with_attributes([
                KeyValue::new("component", "grpc"),
                KeyValue::new("movie.id", id.clone()),
            ])
            .start(&tracer);
        let cx = Context::current_with_span(span);

        let mut client = self.grpc_client.lock().await;
        let mut request = Request::new(DeleteMovieRequest { id });

        self.inject_trace_context(&cx, &mut request);

        let response_result = client.delete_movie(request).await;
        self.add_completion_event(
            &cx,
            &response_result,
            "Delete movie request completed".to_string(),
        );

        match response_result {
            Ok(response) => Ok(response.into_inner().success),
            Err(status) => Err(status),
        }
    }

    fn inject_trace_context<T>(&self, cx: &Context, request: &mut Request<T>) {
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(cx, &mut MetadataMap(request.metadata_mut()))
        });
    }

    fn add_completion_event<T>(
        &self,
        cx: &Context,
        result: &Result<T, Status>,
        event_name: String,
    ) {
        let status = match result {
            Ok(_) => "OK".to_string(),
            Err(status) => status.code().to_string(),
        };

        cx.span()
            .add_event(event_name, vec![KeyValue::new("status", status)]);
    }
}

fn error_response(
    code: axum::http::StatusCode,
    message: &str,
) -> (axum::http::StatusCode, Json<Value>) {
    (code, Json(json!({ "error": message })))
}

pub async fn create_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Json(input): Json<MovieInput>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let state = state.lock().await;

    match state.movie_service.create_movie(input).await {
        Ok(movie) => Ok(Json(json!(movie))),
        Err(status) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

pub async fn get_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let state = state.lock().await;

    match state.movie_service.get_movie(id).await {
        Ok(movie) => Ok(Json(json!(movie))),
        Err(status) => Err(error_response(StatusCode::NOT_FOUND, &status.to_string())),
    }
}

pub async fn list_movies(
    State(state): State<Arc<Mutex<AppState>>>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let state = state.lock().await;

    match state.movie_service.list_movies().await {
        Ok(movies) => Ok(Json(serde_json::to_value(movies).unwrap())),
        Err(status) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

pub async fn update_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(id): Path<String>,
    Json(input): Json<MovieInput>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let state = state.lock().await;

    match state.movie_service.update_movie(id, input).await {
        Ok(movie) => Ok(Json(json!(movie))),
        Err(status) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

pub async fn delete_movie(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    let state = state.lock().await;

    match state.movie_service.delete_movie(id).await {
        Ok(success) => Ok(Json(json!({ "success": success }))),
        Err(status) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &status.to_string(),
        )),
    }
}

pub async fn run_metrics_collector(system_metrics: Arc<SystemMetrics>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        interval.tick().await;
        system_metrics.update_metrics().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tracer_provider = init_tracer();

    global::set_tracer_provider(tracer_provider.clone());

    let metrics = Arc::new(Mutex::new(Metrics {
        requests: Family::default(),
    }));

    let mut registry = Registry::default();

    {
        let metrics_guard = metrics.lock().await;
        registry.register(
            "movie_requests",
            "Total number of movie service requests",
            metrics_guard.requests.clone(),
        );
    }

    let grpc_client = Arc::new(Mutex::new(
        MovieServiceClient::connect("http://movie-server:50051").await?,
    ));

    let system_metrics = Arc::new(SystemMetrics::new());

    system_metrics.register(&mut registry);

    let movie_service = Arc::new(MovieService::new(grpc_client.clone(), metrics.clone()));

    let state = Arc::new(Mutex::new(AppState {
        registry,
        grpc_client: grpc_client.clone(),
        metrics: metrics.clone(),
        system_metrics: system_metrics.clone(),
        movie_service: movie_service.clone(),
    }));

    tokio::spawn(run_metrics_collector(system_metrics.clone()));

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/movies", get(list_movies).post(create_movie))
        .route(
            "/movies/{id}",
            get(get_movie).put(update_movie).delete(delete_movie),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5000").await?;
    println!("Server running on http://0.0.0.0:5000");

    axum::serve(listener, app).await?;

    Ok(())
}
