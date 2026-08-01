use cadx_desktop::CadxApp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CADX")
            .with_inner_size([1480.0, 920.0])
            .with_min_inner_size([1024.0, 680.0]),
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 24,
        multisampling: 4,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "CADX",
        options,
        Box::new(|context| Ok(Box::new(CadxApp::new(context)))),
    )
}
