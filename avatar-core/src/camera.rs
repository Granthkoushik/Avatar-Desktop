use std::process::{Command, Child, Stdio};
use std::io::{Read, Write};
use std::error::Error;
use std::sync::{Arc, Mutex};
use log::{info, error, warn};

const SCRIPT_CONTENT: &str = r#"
import cv2
import sys
import time

def main():
    # Open default webcam
    cap = cv2.VideoCapture(0)
    if not cap.isOpened():
        sys.stderr.write("Error: Could not open camera\n")
        sys.exit(1)
    
    # 640x480 resolution is optimal for low-latency streaming
    cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
    
    try:
        while True:
            ret, frame = cap.read()
            if not ret:
                break
            
            # Encode frame to JPEG (quality 50 for low-latency)
            ret_encode, jpeg_bytes = cv2.imencode('.jpg', frame, [int(cv2.IMWRITE_JPEG_QUALITY), 50])
            if not ret_encode:
                continue
                
            data = jpeg_bytes.tobytes()
            # Write 4-byte big-endian header for length
            sys.stdout.buffer.write(len(data).to_bytes(4, byteorder='big'))
            sys.stdout.buffer.write(data)
            sys.stdout.buffer.flush()
            
            # Sleep to maintain ~20 FPS
            time.sleep(0.05)
    except Exception as e:
        sys.stderr.write(f"Exception: {e}\n")
    finally:
        cap.release()

if __name__ == '__main__':
    main()
"#;

pub struct CameraCapture {
    child: Mutex<Option<Child>>,
}

impl CameraCapture {
    pub fn new() -> Self {
        CameraCapture {
            child: Mutex::new(None),
        }
    }

    /// Spawns the background Python OpenCV capture script.
    pub fn start(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut lock = self.child.lock().unwrap();
        if lock.is_some() {
            return Ok(());
        }

        // Write the embedded python script to the temp directory
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("avatar_camera_capture.py");
        std::fs::write(&script_path, SCRIPT_CONTENT)?;
        
        info!("Spawning camera capture script at {:?}", script_path);
        let child = Command::new("python")
            .arg(script_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        *lock = Some(child);
        Ok(())
    }

    /// Terminates the background capture script.
    pub fn stop(&self) {
        let mut lock = self.child.lock().unwrap();
        if let Some(mut child) = lock.take() {
            info!("Stopping camera capture process...");
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Reads a single JPEG frame from the python stdout pipe.
    pub fn read_frame(&self) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let mut lock = self.child.lock().unwrap();
        if let Some(child) = lock.as_mut() {
            let stdout = child.stdout.as_mut().ok_or("Failed to get camera stdout pipe")?;
            
            // Read 4-byte big-endian length header
            let mut len_bytes = [0u8; 4];
            stdout.read_exact(&mut len_bytes)?;
            let len = u32::from_be_bytes(len_bytes) as usize;
            
            if len > 5 * 1024 * 1024 {
                return Err("Frame size too large (exceeds 5MB limit)".into());
            }

            // Read the exact JPEG bytes
            let mut jpeg_bytes = vec![0u8; len];
            stdout.read_exact(&mut jpeg_bytes)?;
            
            Ok(jpeg_bytes)
        } else {
            Err("Camera not started or already stopped".into())
        }
    }
}
