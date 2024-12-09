## Movie Axum dan Tonic

## Running Server Grpc
```bash
cargo run --bin movie-server
```

## Running Client Axum

```bash
cargo run --bin movie-client
```

-------

### 1. List Movies

```bash
curl -X GET http://127.0.0.1:5000/movies
```

### 2. Create Movie

```bash
curl -X POST http://127.0.0.1:5000/movies \
-H "Content-Type: application/json" \
-d '{
  "title": "Inception",
  "genre": "Sci-Fi"
}'
```

### 3. Get Movie by ID

```bash
curl -X GET http://127.0.0.1:5000/movies/1
```

### 4. Update Movie

```bash
curl -X PUT http://127.0.0.1:5000/movies/1 \
-H "Content-Type: application/json" \
-d '{
  "title": "Interstellar",
  "genre": "Adventure"
}'
```

### 5. Delete Movie

```bash
curl -X DELETE http://127.0.0.1:5000/movies/1
```