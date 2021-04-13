use pilka_lib::*;

#[cfg(debug_assertions)]
#[allow(unused_imports)]
#[allow(clippy::single_component_path_imports)]
use pilka_dyn;

mod audio;
mod default_shaders;
mod input;
mod recorder;

use pilka::create_folder;

use ash::{version::DeviceV1_0, vk, SHADER_ENTRY_POINT, SHADER_PATH};
use eyre::*;
use notify::{
    event::{EventKind, ModifyKind},
    RecommendedWatcher, RecursiveMode, Watcher,
};
use recorder::RecordEvent;
use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
    time::Instant,
};
use winit::{
    dpi::PhysicalPosition,
    dpi::PhysicalSize,
    event::{ElementState, Event, KeyboardInput, VirtualKeyCode, WindowEvent},
    event_loop::ControlFlow,
};

const SCREENSHOTS_FOLDER: &str = "screenshots";
const SHADER_DUMP_FOLDER: &str = "shader_dump";
const VIDEO_FOLDER: &str = "recordings";

fn main() -> Result<()> {
    // Initialize error hook.
    color_eyre::install()?;

    let mut audio_context = audio::AudioContext::new()?;

    let mut input = input::Input::new();
    let mut pause = false;
    let mut time = Instant::now();
    let mut backup_time = time.elapsed();
    let dt = 1. / 60.;

    let event_loop = winit::event_loop::EventLoop::new();

    let window = winit::window::WindowBuilder::new()
        .with_title("Pilka")
        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
        .build(&event_loop)?;

    let mut pilka = PilkaRender::new(&window).unwrap();

    let shader_dir = PathBuf::new().join(SHADER_PATH);

    if !shader_dir.is_dir() {
        default_shaders::create_default_shaders(&shader_dir)?;
    }

    pilka.push_render_pipeline(
        ash::ShaderInfo::new(shader_dir.join("shader.vert"), SHADER_ENTRY_POINT.into())?,
        ash::ShaderInfo::new(shader_dir.join("shader.frag"), SHADER_ENTRY_POINT.into())?,
        &[shader_dir.join("prelude.glsl")],
    )?;

    pilka.push_compute_pipeline(
        ash::ShaderInfo::new(shader_dir.join("shader.comp"), SHADER_ENTRY_POINT.into())?,
        &[],
    )?;

    let (ffmpeg_version, has_ffmpeg) = recorder::ffmpeg_version()?;

    println!("Vendor name: {}", pilka.get_vendor_name());
    println!("Device name: {}", pilka.get_device_name()?);
    println!("Device type: {:?}", pilka.get_device_type());
    println!("Vulkan version: {}", pilka.get_vulkan_version_name()?);
    println!("Audio host: {:?}", audio_context.host_id);
    println!(
        "Sample rate: {}, channels: {}",
        audio_context.sample_rate, audio_context.num_channels
    );
    println!("{}", ffmpeg_version);
    println!(
        "Default shader path:\n\t{}",
        shader_dir.canonicalize()?.display()
    );

    print_help();

    println!("// Set up our new world⏎ ");
    println!("// And let's begin the⏎ ");
    println!("\tSIMULATION⏎ \n");

    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher: RecommendedWatcher = Watcher::new_immediate(move |res| match res {
        Ok(event) => {
            tx.send(event).unwrap();
        }
        Err(e) => println!("watch error: {:?}", e),
    })?;

    watcher.watch(SHADER_PATH, RecursiveMode::Recursive)?;

    let mut video_recording = false;
    let (video_tx, video_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || recorder::record_thread(video_rx));

    event_loop.run(move |event, _, control_flow| {
        *control_flow = winit::event_loop::ControlFlow::Poll;
        match event {
            Event::NewEvents(_) => {
                if let Ok(rx_event) = rx.try_recv() {
                    if let notify::Event {
                        kind: EventKind::Modify(ModifyKind::Data(_)),
                        ..
                    } = rx_event
                    {
                        unsafe { pilka.device.device_wait_idle() }.unwrap();
                        for path in rx_event.paths {
                            if pilka.shader_set.contains_key(&path) {
                                pilka.rebuild_pipeline(pilka.shader_set[&path]).unwrap();
                            }
                        }
                    }
                }

                pilka.paused = !pause;

                pilka.push_constant.time = if pause {
                    backup_time.as_secs_f32()
                } else {
                    time.elapsed().as_secs_f32()
                };

                if !pause {
                    let mut tmp_buf = [0f32; audio::FFT_SIZE];
                    audio_context.get_fft(&mut tmp_buf);
                    pilka.update_fft_texture(&tmp_buf).unwrap();

                    input.process_position(&mut pilka.push_constant);
                }
                pilka.push_constant.wh = pilka.surface.resolution_slice(&pilka.device).unwrap();
            }

            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::Resized(PhysicalSize { .. }) => {
                    let vk::Extent2D { width, height } =
                        pilka.surface.resolution(&pilka.device).unwrap();
                    let vk::Extent2D {
                        width: old_width,
                        height: old_height,
                    } = pilka.extent;

                    if width == old_width && height == old_height {
                        return;
                    }

                    pilka.resize().unwrap();

                    if video_recording {
                        println!(
                            "Stop recording. Resolution has been changed {}×{} => {}×{}.",
                            width, height, old_width, old_height
                        );
                        video_recording = false;
                        video_tx.send(RecordEvent::Finish).unwrap();
                    }
                }
                WindowEvent::KeyboardInput {
                    input:
                        KeyboardInput {
                            virtual_keycode: Some(keycode),
                            state,
                            ..
                        },
                    ..
                } => {
                    input.update(&keycode, &state);

                    if VirtualKeyCode::Escape == keycode {
                        *control_flow = ControlFlow::Exit;
                    }

                    if ElementState::Pressed == state {
                        if VirtualKeyCode::F1 == keycode {
                            print_help();
                        }

                        if VirtualKeyCode::F2 == keycode {
                            if !pause {
                                backup_time = time.elapsed();
                                pause = true;
                            } else {
                                time = Instant::now() - backup_time;
                                pause = false;
                            }
                        }

                        if VirtualKeyCode::F3 == keycode {
                            if !pause {
                                backup_time = time.elapsed();
                                pause = true;
                            }
                            backup_time = backup_time
                                .checked_sub(std::time::Duration::from_secs_f32(dt))
                                .unwrap_or_else(Default::default);
                        }

                        if VirtualKeyCode::F4 == keycode {
                            if !pause {
                                backup_time = time.elapsed();
                                pause = true;
                            }
                            backup_time += std::time::Duration::from_secs_f32(dt);
                        }

                        if VirtualKeyCode::F5 == keycode {
                            pilka.push_constant.pos = [0.; 3];
                            pilka.push_constant.time = 0.;
                            time = Instant::now();
                            backup_time = time.elapsed();
                        }

                        if VirtualKeyCode::F6 == keycode {
                            eprintln!("{}", pilka.push_constant);
                        }

                        if VirtualKeyCode::F10 == keycode {
                            save_shaders(&pilka).unwrap();
                        }

                        if VirtualKeyCode::F11 == keycode {
                            let now = Instant::now();
                            let (_, (width, height)) = pilka.capture_frame().unwrap();
                            eprintln!("Capture image: {:#?}", now.elapsed());
                            let frame = &pilka.screenshot_ctx.data[..(width * height * 4) as usize];
                            save_screenshot(frame, width, height);
                        }

                        if has_ffmpeg && VirtualKeyCode::F12 == keycode {
                            if video_recording {
                                video_tx.send(RecordEvent::Finish).unwrap()
                            } else {
                                let (_, (w, h)) = pilka.capture_frame().unwrap();
                                video_tx
                                    .send(RecordEvent::Start(w as u32, h as u32))
                                    .unwrap()
                            }
                            video_recording = !video_recording;
                        }
                    }
                }

                WindowEvent::CursorMoved {
                    position: PhysicalPosition { x, y },
                    ..
                } => {
                    if !pause {
                        let vk::Extent2D { width, height } = pilka.extent;
                        let x = (x as f32 / width as f32 - 0.5) * 2.;
                        let y = -(y as f32 / height as f32 - 0.5) * 2.;
                        pilka.push_constant.mouse = [x, y];
                    }
                }
                WindowEvent::MouseInput {
                    button: winit::event::MouseButton::Left,
                    state,
                    ..
                } => match state {
                    ElementState::Pressed => pilka.push_constant.mouse_pressed = true as _,
                    ElementState::Released => pilka.push_constant.mouse_pressed = false as _,
                },
                _ => {}
            },

            Event::MainEventsCleared => {
                pilka.render().unwrap();
                if video_recording {
                    let (frame, _) = pilka.capture_frame().unwrap();
                    video_tx.send(RecordEvent::Record(frame.to_vec())).unwrap()
                }
            }
            Event::LoopDestroyed => {
                println!("// End from the loop. Bye bye~⏎ ");
                unsafe { pilka.device.device_wait_idle() }.unwrap();
            }
            _ => {}
        }
    });
}

fn print_help() {
    println!("\n- `F1`:   Print help");
    println!("- `F2`:   Toggle play/pause");
    println!("- `F3`:   Pause and step back one frame");
    println!("- `F4`:   Pause and step forward one frame");
    println!("- `F5`:   Restart playback at frame 0 (`Time` and `Pos` = 0)");
    println!("- `F6`:   Print parameters");
    println!("- `F10`:  Save shaders");
    println!("- `F11`:  Take Screenshot");
    println!("- `F12`:  Start/Stop record video");
    println!("- `ESC`:  Exit the application");
    println!("- `Arrows`: Change `Pos`\n");
}

fn save_screenshot(
    frame: &'static [u8],
    width: u32,
    height: u32,
) -> std::thread::JoinHandle<Result<()>> {
    std::thread::spawn(move || {
        let now = Instant::now();
        let screenshots_folder = Path::new(SCREENSHOTS_FOLDER);
        create_folder(screenshots_folder)?;
        let path = screenshots_folder.join(format!(
            "screenshot-{}.png",
            chrono::Local::now().format("%d-%m-%Y-%H-%M-%S").to_string()
        ));
        let file = File::create(path)?;
        let w = BufWriter::new(file);
        let mut encoder = png::Encoder::new(w, width, height);
        encoder.set_color(png::ColorType::RGBA);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(frame)?;
        eprintln!("Encode image: {:#?}", now.elapsed());
        Ok(())
    })
}

fn save_shaders(pilka: &PilkaRender) -> Result<()> {
    let dump_folder = std::path::Path::new(SHADER_DUMP_FOLDER);
    create_folder(dump_folder)?;
    let dump_folder =
        dump_folder.join(chrono::Local::now().format("%d-%m-%Y-%H-%M-%S").to_string());
    create_folder(&dump_folder)?;
    let dump_folder = dump_folder.join(SHADER_PATH);
    create_folder(&dump_folder)?;

    for path in pilka.shader_set.keys() {
        let to = dump_folder.join(path.strip_prefix(Path::new(SHADER_PATH).canonicalize()?)?);
        if !to.exists() {
            std::fs::create_dir_all(&to.parent().unwrap().canonicalize()?)?;
            std::fs::File::create(&to)?;
        }
        std::fs::copy(path, &to)?;
        eprintln!("Saved: {}", &to.display());
    }

    Ok(())
}
