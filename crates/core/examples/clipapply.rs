//! Test helper: apply clipboard contents the way OpenHop does when they
//! arrive from another computer, then keep serving them.
//! `clipapply files A B`, `clipapply png F`, `clipapply text T`
use openhop_core::protocol::ClipData;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = match args[1].as_str() {
        "files" => ClipData::Files(args[2..].to_vec()),
        "png" => ClipData::Png(std::fs::read(&args[2]).unwrap()),
        _ => ClipData::Text(args[2].clone()),
    };
    let (tx, _rx) = openhop_core::clipboard::start();
    tx.send(data).unwrap();
    eprintln!("applied");
    std::thread::sleep(std::time::Duration::from_secs(300));
}
