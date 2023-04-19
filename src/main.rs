use std::{sync::Arc, time::Duration};

use bit::Bit;
use tokio::time::Instant;

pub mod bit;
pub mod op;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cpu = Arc::new(Bit::hi() & Bit::hi());
    loop {
        let deadline = Instant::now() + Duration::from_millis(100);
        let c = cpu.clone().eval().await;
        println!("{}", c);

        tokio::time::sleep_until(deadline).await;
    }
}
