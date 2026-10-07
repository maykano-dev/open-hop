//! Test helper: put text or a PNG on the clipboard and keep serving it.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut cb = arboard::Clipboard::new().unwrap();
    match args[1].as_str() {
        "text" => cb.set_text(args[2].clone()).unwrap(),
        "files" => cb.set().file_list(&args[2..]).unwrap(),
        _ => {
            let data = std::fs::read(&args[2]).unwrap();
            let (w, h, rgba) = openhop_core::clipboard::decode_png(&data).unwrap();
            cb.set_image(arboard::ImageData { width: w, height: h, bytes: rgba.into() }).unwrap();
        }
    }
    std::thread::sleep(std::time::Duration::from_secs(120));
}
