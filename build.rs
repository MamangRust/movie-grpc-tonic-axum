fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/movie.proto")?;
    Ok(())
}

// fn main() -> Result<(), Box<dyn std::error::Error>> {
//     let protos = [
//         "proto/movie.proto",
//         "proto/user.proto",
//         "proto/auth.proto",
//     ];

//     tonic_build::configure()
//         .compile_protos(&protos, &["proto/"])?; 

//     Ok(())
// }
