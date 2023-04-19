use std::{sync::Arc, time::Duration};

use bit::Bit;
use context::ExecutionContext;
use tokio::time::Instant;

pub mod bit;
pub mod context;
pub mod op;
pub mod parser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // let cpu = Arc::new(Bit::hi() & Bit::hi());
    let mut cpus = ExecutionContext::parse_script(
        include_str!("parser/test_scripts/test1.pals"),
        vec![vec![Bit::hi().into(), Bit::lo().into(), Bit::lo().into()]],
    )?;
    loop {
        let deadline = Instant::now() + Duration::from_millis(100);
        // let c = cpu.clone().eval().await;
        let mut outputs = vec![];
        for cpu in cpus.iter_mut() {
            outputs.push(cpu.exec().await);
        }
        println!("{:?}", outputs);

        tokio::time::sleep_until(deadline).await;
    }
}
