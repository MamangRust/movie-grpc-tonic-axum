use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status, transport::Server};
use uuid::Uuid;

// This module will contain the generated protobuf code
pub mod movie {
    tonic::include_proto!("movie");
}
use movie::movie_service_server::MovieService;

// Import the generated types
use movie::{
    Movie, 
    CreateMovieRequest, 
    CreateMovieResponse,
    ReadMovieRequest, 
    ReadMovieResponse,
    ReadMoviesRequest, 
    ReadMoviesResponse,
    UpdateMovieRequest, 
    UpdateMovieResponse,
    DeleteMovieRequest, 
    DeleteMovieResponse
};

// In-memory movie storage
#[derive(Debug, Default, Clone)]
struct MovieStore {
    movies: Arc<Mutex<HashMap<String, Movie>>>
}

#[derive(Debug, Default)]
pub struct MovieServiceImpl {
    store: MovieStore
}

#[tonic::async_trait]
impl MovieService for MovieServiceImpl {
    async fn create_movie(
        &self, 
        request: Request<CreateMovieRequest>
    ) -> Result<Response<CreateMovieResponse>, Status> {
        let mut movies = self.store.movies.lock().map_err(|_| Status::internal("Lock error"))?;
        
        let mut movie = request.into_inner().movie.ok_or(Status::invalid_argument("No movie provided"))?;
        
        // Generate a UUID if no ID is provided
        if movie.id.is_empty() {
            movie.id = Uuid::new_v4().to_string();
        }
        
        // Insert the movie
        movies.insert(movie.id.clone(), movie.clone());
        
        Ok(Response::new(CreateMovieResponse { movie: Some(movie) }))
    }

    async fn get_movie(
        &self, 
        request: Request<ReadMovieRequest>
    ) -> Result<Response<ReadMovieResponse>, Status> {
        let movies = self.store.movies.lock().map_err(|_| Status::internal("Lock error"))?;
        
        let id = request.into_inner().id;
        
        let movie = movies.get(&id)
            .cloned()
            .ok_or_else(|| Status::not_found("Movie not found"))?;
        
        Ok(Response::new(ReadMovieResponse { movie: Some(movie) }))
    }

    async fn get_movies(
        &self, 
        _request: Request<ReadMoviesRequest>
    ) -> Result<Response<ReadMoviesResponse>, Status> {
        let movies = self.store.movies.lock().map_err(|_| Status::internal("Lock error"))?;
        
        let movie_list = movies.values().cloned().collect();
        
        Ok(Response::new(ReadMoviesResponse { movies: movie_list }))
    }

    async fn update_movie(
        &self, 
        request: Request<UpdateMovieRequest>
    ) -> Result<Response<UpdateMovieResponse>, Status> {
        let mut movies = self.store.movies.lock().map_err(|_| Status::internal("Lock error"))?;
        
        let movie = request.into_inner().movie.ok_or(Status::invalid_argument("No movie provided"))?;
        
        if !movies.contains_key(&movie.id) {
            return Err(Status::not_found("Movie not found"));
        }
        
        movies.insert(movie.id.clone(), movie.clone());
        
        Ok(Response::new(UpdateMovieResponse { movie: Some(movie) }))
    }

    async fn delete_movie(
        &self, 
        request: Request<DeleteMovieRequest>
    ) -> Result<Response<DeleteMovieResponse>, Status> {
        let mut movies = self.store.movies.lock().map_err(|_| Status::internal("Lock error"))?;
        
        let id = request.into_inner().id;
        
        let removed = movies.remove(&id).is_some();
        
        Ok(Response::new(DeleteMovieResponse { success: removed }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse()?;
    
    let movie_service = MovieServiceImpl::default();
    
    println!("Movie Service listening on {}", addr);
    
    Server::builder()
        .add_service(movie::movie_service_server::MovieServiceServer::new(movie_service))
        .serve(addr)
        .await?;
    
    Ok(())
}