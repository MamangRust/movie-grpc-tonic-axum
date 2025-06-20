use opentelemetry::{
    global,
    propagation::Extractor,
    trace::{Span, SpanKind, Tracer},
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, error::Error, sync::OnceLock};
use tonic::{transport::Server, Request, Response, Status};
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use movie::{
    movie_service_server::MovieService, CreateMovieRequest, CreateMovieResponse,
    DeleteMovieRequest, DeleteMovieResponse, Movie, ReadMovieRequest, ReadMovieResponse,
    ReadMoviesRequest, ReadMoviesResponse, UpdateMovieRequest, UpdateMovieResponse,
};

pub mod movie {
    tonic::include_proto!("movie");
}

struct MetadataMap<'a>(&'a tonic::metadata::MetadataMap);

impl<'a> Extractor for MetadataMap<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|metadata| metadata.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .map(|key| match key {
                tonic::metadata::KeyRef::Ascii(v) => v.as_str(),
                tonic::metadata::KeyRef::Binary(v) => v.as_str(),
            })
            .collect::<Vec<_>>()
    }
}

pub struct Telemetry;

impl Telemetry {
    fn get_resource() -> Resource {
        static RESOURCE: OnceLock<Resource> = OnceLock::new();
        RESOURCE
            .get_or_init(|| {
                Resource::builder()
                    .with_service_name("movie-server")
                    .build()
            })
            .clone()
    }

    pub fn init_tracer() -> SdkTracerProvider {
        let exporter = SpanExporter::builder()
            .with_tonic()
            .with_endpoint("http://otel-collector:4317")
            .build()
            .expect("Failed to create span exporter");
        SdkTracerProvider::builder()
            .with_resource(Self::get_resource())
            .with_batch_exporter(exporter)
            .build()
    }

    pub fn init_meter() -> SdkMeterProvider {
        let exporter = MetricExporter::builder()
            .with_tonic()
            .with_endpoint("http://otel-collector:4317")
            .build()
            .expect("Failed to create metric exporter");

        SdkMeterProvider::builder()
            .with_periodic_exporter(exporter)
            .with_resource(Self::get_resource())
            .build()
    }

    pub fn init_logger() -> SdkLoggerProvider {
        let exporter = LogExporter::builder()
            .with_tonic()
            .with_endpoint("http://otel-collector:4317")
            .build()
            .expect("Failed to create log exporter");

        SdkLoggerProvider::builder()
            .with_resource(Self::get_resource())
            .with_batch_exporter(exporter)
            .build()
    }
}

#[derive(Debug, Default, Clone)]
struct MovieStore {
    movies: Arc<Mutex<HashMap<String, Movie>>>,
}

#[derive(Debug, Default)]
pub struct MovieServiceImpl {
    store: MovieStore,
}

#[tonic::async_trait]
impl MovieService for MovieServiceImpl {
    async fn create_movie(
        &self,
        request: Request<CreateMovieRequest>,
    ) -> Result<Response<CreateMovieResponse>, Status> {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie-server");
        let mut span = tracer
            .span_builder("CreateMovie")
            .with_kind(SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let mut movies = self
            .store
            .movies
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let mut movie = request
            .into_inner()
            .movie
            .ok_or(Status::invalid_argument("No movie provided"))?;

        if movie.id.is_empty() {
            movie.id = Uuid::new_v4().to_string();
            span.add_event(format!("Generated new movie ID: {}", movie.id), vec![]);
        }

        movies.insert(movie.id.clone(), movie.clone());

        span.add_event("Movie created successfully", vec![]);

        Ok(Response::new(CreateMovieResponse { movie: Some(movie) }))
    }

    async fn get_movie(
        &self,
        request: Request<ReadMovieRequest>,
    ) -> Result<Response<ReadMovieResponse>, Status> {
        // Extract parent context and create span
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie-server");
        let mut span = tracer
            .span_builder("GetMovie")
            .with_kind(SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let movies = self
            .store
            .movies
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let id = request.into_inner().id;
        span.add_event(format!("Fetching movie with ID: {}", id), vec![]);

        let movie = movies
            .get(&id)
            .cloned()
            .ok_or_else(|| Status::not_found("Movie not found"))?;

        span.add_event("Movie retrieved successfully", vec![]);

        Ok(Response::new(ReadMovieResponse { movie: Some(movie) }))
    }

    async fn get_movies(
        &self,
        request: Request<ReadMoviesRequest>,
    ) -> Result<Response<ReadMoviesResponse>, Status> {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie-server");
        let mut span = tracer
            .span_builder("GetMovies")
            .with_kind(SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let movies = self
            .store
            .movies
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let movie_list: Vec<Movie> = movies.values().cloned().collect();

        span.add_event(format!("Retrieved {} movies", movie_list.len()), vec![]);

        Ok(Response::new(ReadMoviesResponse { movies: movie_list }))
    }

    async fn update_movie(
        &self,
        request: Request<UpdateMovieRequest>,
    ) -> Result<Response<UpdateMovieResponse>, Status> {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie-server");
        let mut span = tracer
            .span_builder("UpdateMovie")
            .with_kind(SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let mut movies = self
            .store
            .movies
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let movie = request
            .into_inner()
            .movie
            .ok_or(Status::invalid_argument("No movie provided"))?;

        if !movies.contains_key(&movie.id) {
            span.add_event(format!("Movie not found: {}", movie.id), vec![]);
            return Err(Status::not_found("Movie not found"));
        }

        movies.insert(movie.id.clone(), movie.clone());

        span.add_event(format!("Movie updated: {}", movie.id), vec![]);

        Ok(Response::new(UpdateMovieResponse { movie: Some(movie) }))
    }

    async fn delete_movie(
        &self,
        request: Request<DeleteMovieRequest>,
    ) -> Result<Response<DeleteMovieResponse>, Status> {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie-server");
        let mut span = tracer
            .span_builder("DeleteMovie")
            .with_kind(SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let mut movies = self
            .store
            .movies
            .lock()
            .map_err(|_| Status::internal("Lock error"))?;

        let id = request.into_inner().id;

        let removed = movies.remove(&id).is_some();

        span.add_event(
            format!("Delete movie operation: ID = {}, Success = {}", id, removed),
            vec![],
        );

        Ok(Response::new(DeleteMovieResponse { success: removed }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let tracer_provider = Telemetry::init_tracer();
    let meter_provider = Telemetry::init_meter();
    let logger_provider = Telemetry::init_logger();

    let otel_layer = OpenTelemetryTracingBridge::new(&logger_provider);

    let filter_otel = EnvFilter::new("info")
        .add_directive("hyper=off".parse().unwrap())
        .add_directive("tonic=off".parse().unwrap())
        .add_directive("h2=off".parse().unwrap())
        .add_directive("reqwest=off".parse().unwrap());
    let otel_layer = otel_layer.with_filter(filter_otel);

    let filter_fmt = EnvFilter::new("info").add_directive("opentelemetry=debug".parse().unwrap());
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_thread_names(true)
        .with_filter(filter_fmt);

    tracing_subscriber::registry()
        .with(otel_layer)
        .with(fmt_layer)
        .init();

    global::set_tracer_provider(tracer_provider.clone());
    global::set_meter_provider(meter_provider.clone());

    let addr = "0.0.0.0:50051".parse()?;
    let movie_service = MovieServiceImpl::default();

    println!("Movie Service listening on {}", addr);


    Server::builder()
        .add_service(movie::movie_service_server::MovieServiceServer::new(
            movie_service,
        ))
        .serve(addr)
        .await?;


    let mut shutdown_errors = Vec::new();
    if let Err(e) = tracer_provider.shutdown() {
        shutdown_errors.push(format!("tracer provider: {}", e));
    }

    if let Err(e) = meter_provider.shutdown() {
        shutdown_errors.push(format!("meter provider: {}", e));
    }

    if let Err(e) = logger_provider.shutdown() {
        shutdown_errors.push(format!("logger provider: {}", e));
    }

    if !shutdown_errors.is_empty() {
        return Err(format!(
            "Failed to shutdown providers:\n{}",
            shutdown_errors.join("\n")
        )
        .into());
    }

    Ok(())
}
