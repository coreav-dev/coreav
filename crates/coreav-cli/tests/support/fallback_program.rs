use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    if let Ok(started) = std::env::var("FALLBACK_TEST_STARTED") {
        std::fs::write(started, b"started").unwrap();
        let release = std::env::var("FALLBACK_TEST_RELEASE").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !Path::new(&release).exists() {
            if Instant::now() >= deadline {
                std::process::exit(99);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    println!("args: {}", args.join(" "));
    eprintln!("guard={}", std::env::var("COREAV_FALLBACK_ACTIVE").unwrap());
    std::io::stdout().flush().unwrap();
    std::process::exit(
        std::env::var("FALLBACK_TEST_EXIT")
            .unwrap()
            .parse()
            .unwrap(),
    );
}
