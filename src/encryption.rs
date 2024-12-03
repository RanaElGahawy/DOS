use std::io::{self, Cursor};
use std::path::Path;
use image::{DynamicImage, ImageFormat};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use std::sync::Arc;
use steganography::encoder;

pub async fn encode_and_send(
    file_name: String,
    encoded_file_name: String,
    socket: &mut TcpStream,
    request_count: Arc<Mutex<u32>>,
 ) -> io::Result<()> {
    println!("Starting encoding for: {}", file_name);
 
    // Step 1: Encode the image
    let encoded_image_path = match encode_image_with_hidden(&file_name) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error encoding image {}: {:?}", file_name, e);
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }
    };
 
    println!("Encoding completed. Sending back encoded image: {}", encoded_image_path);
 
    // Step 2: Send the encoded image back to the client
    let mut file = fs::File::open(&encoded_image_path).await?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).await?;
 
    socket.write_all(&buffer).await?;
    println!("Sent encoded image: {}", encoded_file_name);
 
    // Delete the images after sending
    if let Err(e) = fs::remove_file(file_name.clone()).await {  // Clone file_name to avoid moving it
        eprintln!("Failed to delete original image {}: {}", file_name, e);
    }
    if let Err(e) = fs::remove_file(encoded_image_path).await {
        eprintln!("Failed to delete encoded image: {}", e);
    }
 
    // Decrement request_count when handling completes
    {
        let mut count = request_count.lock().await;
        *count -= 1;
        println!("Request count decremented to: {}", *count);
    }
 
    Ok(())
}

fn encode_image_with_hidden(hide_image_path: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
   let hidden_image_path = Path::new(hide_image_path);
   if !hidden_image_path.exists() {
       return Err(format!("File not found: {}", hide_image_path).into());
   }

   // Open the image file
   let hidden_image = image::open(&hidden_image_path)?.to_rgba();

   // Resize hidden image to fit in the cover image (half of its size)
   let resized_hidden_image = image::imageops::resize(
       &hidden_image,
       hidden_image.width() / 2,
       hidden_image.height() / 2,
       image::imageops::FilterType::Lanczos3,
   );

   let mut hidden_image_bytes: Vec<u8> = Vec::new();
   DynamicImage::ImageRgba8(resized_hidden_image)
       .write_to(&mut Cursor::new(&mut hidden_image_bytes), ImageFormat::JPEG)?;

   // Load the cover image
   let cover_image_path = Path::new("./default.png");
   if !cover_image_path.exists() {
       return Err("Cover image (default.png) not found.".into());
   }

   let default_image = image::open(cover_image_path)?.to_rgba();

   // Resize cover image to match the hidden image size
   let resized_cover_image = image::imageops::resize(
       &default_image,
       hidden_image.width(),
       hidden_image.height(),
       image::imageops::FilterType::Lanczos3,
   );

   // Encode the image using steganography
   let encoder = encoder::Encoder::new(&hidden_image_bytes, DynamicImage::ImageRgba8(resized_cover_image));
   let encoded_image = encoder.encode_alpha();

   // Save the encoded image
   let encoded_image_path = format!("{}_encoded.png", hide_image_path);
   encoded_image.save(&encoded_image_path)?;

   Ok(encoded_image_path)
}
