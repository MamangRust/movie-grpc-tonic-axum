use axum::{
    routing::{get, post, put, delete},
    Router,
    extract::{State, Path},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use serde::{Serialize, Deserialize};
use tonic::Request;
use std::sync::Arc;
use uuid::Uuid;

pub mod movie {
    tonic::include_proto!("movie");
}

// Import gRPC client
use movie::movie_service_client::MovieServiceClient;
use movie::{CreateMovieRequest, ReadMovieRequest, UpdateMovieRequest, DeleteMovieRequest};

#[derive(Clone)]
struct AppState {
    grpc_client: Arc<tokio::sync::Mutex<MovieServiceClient<tonic::transport::Channel>>>
}


fn error_response(code: axum::http::StatusCode, message: &str) -> (axum::http::StatusCode, Json<Value>) {
    (code, Json(json!({ "error": message })))
}


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



async fn create_movie(
    State(state): State<AppState>, 
    Json(input): Json<MovieInput>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    let mut client = state.grpc_client.lock().await;

    let movie_id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let request = Request::new(CreateMovieRequest { 
        movie: Some(movie::Movie {
            id: movie_id.clone(),
            title: input.title,
            genre: input.genre,
        }),
    });

    match client.create_movie(request).await {
        Ok(response) => {
            let movie = response.into_inner().movie.unwrap();
            Ok(Json(json!({ "id": movie.id, "title": movie.title, "genre": movie.genre })))
        }
        Err(status) => Err(error_response(axum::http::StatusCode::INTERNAL_SERVER_ERROR, &status.to_string())),
    }
}

async fn get_movie(
    State(state): State<AppState>, 
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    let mut client = state.grpc_client.lock().await;

    let request = Request::new(ReadMovieRequest { id });

    match client.get_movie(request).await {
        Ok(response) => {
            let movie = response.into_inner().movie.unwrap();
            Ok(Json(json!({ "id": movie.id, "title": movie.title, "genre": movie.genre })))
        }
        Err(status) => Err(error_response(axum::http::StatusCode::NOT_FOUND, &status.to_string())),
    }
}

async fn list_movies(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    let mut client = state.grpc_client.lock().await;
    let request = Request::new(movie::ReadMoviesRequest {});
    match client.get_movies(request).await {
        Ok(response) => {
            let movies: Vec<movie::Movie> = response.into_inner().movies;
            let movie_responses: Vec<MovieResponse> = movies.into_iter().map(|movie| MovieResponse {
                id: movie.id,
                title: movie.title,
                genre: movie.genre,
                // Map other fields
            }).collect();
            Ok(Json(serde_json::to_value(movie_responses).unwrap()))
        }
        Err(status) => Err(error_response(axum::http::StatusCode::INTERNAL_SERVER_ERROR, &status.to_string())),
    }
}

async fn update_movie(
    State(state): State<AppState>, 
    Path(id): Path<String>,
    Json(input): Json<MovieInput>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    let mut client = state.grpc_client.lock().await;

    let request = Request::new(UpdateMovieRequest { 
        movie: Some(movie::Movie {
            id,
            title: input.title,
            genre: input.genre,
        }),
    });

    match client.update_movie(request).await {
        Ok(response) => {
            let movie = response.into_inner().movie.unwrap();
            Ok(Json(json!({ "id": movie.id, "title": movie.title, "genre": movie.genre })))
        }
        Err(status) => Err(error_response(axum::http::StatusCode::INTERNAL_SERVER_ERROR, &status.to_string())),
    }
}

async fn delete_movie(
    State(state): State<AppState>, 
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<Value>)> {
    let mut client = state.grpc_client.lock().await;

    let request = Request::new(DeleteMovieRequest { id });

    match client.delete_movie(request).await {
        Ok(response) => Ok(Json(json!({ "success": response.into_inner().success }))),
        Err(status) => Err(error_response(axum::http::StatusCode::INTERNAL_SERVER_ERROR, &status.to_string())),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let grpc_client = MovieServiceClient::connect("http://[::1]:50051").await?;
    
    let state = AppState {
        grpc_client: Arc::new(tokio::sync::Mutex::new(grpc_client)),
    };

    let app = Router::new()
        .route("/movies", 
            get(list_movies)
            .post(create_movie)
        )
        .route("/movies/:id", 
            get(get_movie)
            .put(update_movie)
            .delete(delete_movie)
        )
        .with_state(state);

   
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("Server running on http://127.0.0.1:3000");
    
    axum::serve(listener, app).await?;

    Ok(())
}
