use opentelemetry::{
    global,
    propagation::Extractor,
    trace::{Span, SpanKind, Tracer},
};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator, runtime::Tokio, trace::TracerProvider,
};
use opentelemetry_stdout::SpanExporter;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tonic::{transport::Server, Request, Response, Status};
use uuid::Uuid;

use movie::{
    movie_service_server::MovieService, CreateMovieRequest, CreateMovieResponse,
    DeleteMovieRequest, DeleteMovieResponse, Movie, ReadMovieRequest, ReadMovieResponse,
    ReadMoviesRequest, ReadMoviesResponse, UpdateMovieRequest, UpdateMovieResponse,
};

// Import the generated types
pub mod movie {
    tonic::include_proto!("movie");
}

// Metadata extraction for OpenTelemetry
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

// Initialize OpenTelemetry tracer
fn init_tracer() -> TracerProvider {
    global::set_text_map_propagator(TraceContextPropagator::new());
    let provider = TracerProvider::builder()
        .with_batch_exporter(SpanExporter::default(), Tokio)
        .build();

    global::set_tracer_provider(provider.clone());
    provider
}

// In-memory movie storage
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
        // Extract parent context and create span
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie_service/create_movie");
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

        // Generate a UUID if no ID is provided
        if movie.id.is_empty() {
            movie.id = Uuid::new_v4().to_string();
            span.add_event(format!("Generated new movie ID: {}", movie.id), vec![]);
        }

        // Insert the movie
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
        let tracer = global::tracer("movie_service/get_movie");
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
        let tracer = global::tracer("movie_service/get_movies");
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
        // Extract parent context and create span
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie_service/update_movie");
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
        // Extract parent context and create span
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        let tracer = global::tracer("movie_service/delete_movie");
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize OpenTelemetry tracer
    let _provider = init_tracer();

    let addr = "[::1]:50051".parse()?;

    let movie_service = MovieServiceImpl::default();

    println!("Movie Service listening on {}", addr);

    Server::builder()
        .add_service(movie::movie_service_server::MovieServiceServer::new(
            movie_service,
        ))
        .serve(addr)
        .await?;

    Ok(())
}
