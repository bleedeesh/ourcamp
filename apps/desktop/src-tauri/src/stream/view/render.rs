//! Draws pictures of a stream with `wgpu` (ADR-0053): the planes of I420 go into three
//! textures and a shader turns them into RGB, so nothing is converted on the CPU.

use livekit::webrtc::prelude::I420Buffer;

/// A place inside a surface, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The largest place for a picture inside a surface with its proportions kept, centred:
/// what is left over stays black.
pub fn fit(picture: (u32, u32), surface: (u32, u32)) -> Place {
    #[allow(clippy::cast_precision_loss, reason = "sizes of pictures fit in f32")]
    let (pw, ph, sw, sh) = (
        picture.0 as f32,
        picture.1 as f32,
        surface.0 as f32,
        surface.1 as f32,
    );
    let scale = (sw / pw).min(sh / ph);
    // Whole pixels: a viewport between them would blur the picture for no reason.
    let (width, height) = ((pw * scale).round(), (ph * scale).round());
    Place {
        x: ((sw - width) / 2.0).floor(),
        y: ((sh - height) / 2.0).floor(),
        width,
        height,
    }
}

/// Textures of the current picture.
struct Planes {
    size: (u32, u32),
    textures: [wgpu::Texture; 3],
    bind_group: wgpu::BindGroup,
}

/// The GPU's side of a stream's picture, without any window: it draws into whatever
/// view it is given in `format`.
pub struct Painter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    planes: Option<Planes>,
}

impl Painter {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("i420.wgsl"));
        let plane = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stream planes"),
            entries: &[
                plane(0),
                plane(1),
                plane(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("stream"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stream"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stream"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Self {
            device,
            queue,
            pipeline,
            layout,
            sampler,
            planes: None,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Takes a new picture. The textures live until its size changes.
    pub fn upload(&mut self, picture: &I420Buffer) {
        use livekit::webrtc::video_frame::VideoBuffer;

        let size = (picture.width(), picture.height());
        if self.planes.as_ref().is_none_or(|p| p.size != size) {
            self.planes = Some(self.make_planes(size));
        }
        let Some(planes) = &self.planes else { return };
        let (y, u, v) = picture.data();
        let (y_stride, u_stride, v_stride) = picture.strides();
        for (texture, data, stride) in [
            (&planes.textures[0], y, y_stride),
            (&planes.textures[1], u, u_stride),
            (&planes.textures[2], v, v_stride),
        ] {
            self.queue.write_texture(
                texture.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
                texture.size(),
            );
        }
    }

    fn make_planes(&self, size: (u32, u32)) -> Planes {
        let plane = |width, height| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("stream plane"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        // I420 keeps one colour sample for every 2×2 pixels, rounded up.
        let chroma = (size.0.div_ceil(2), size.1.div_ceil(2));
        let textures = [
            plane(size.0, size.1),
            plane(chroma.0, chroma.1),
            plane(chroma.0, chroma.1),
        ];
        let views = textures
            .each_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stream planes"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Planes {
            size,
            textures,
            bind_group,
        }
    }

    /// Draws the current picture into `target`, black around it.
    pub fn draw(&self, target: &wgpu::TextureView, size: (u32, u32)) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stream"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(planes) = &self.planes {
                let place = fit(planes.size, size);
                pass.set_viewport(place.x, place.y, place.width, place.height, 0.0, 1.0);
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &planes.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_picture_gets_bars_above_and_below() {
        let place = fit((1920, 1080), (1000, 1000));
        assert_eq!(
            place,
            Place {
                x: 0.0,
                y: 218.0,
                width: 1000.0,
                height: 563.0
            }
        );
    }

    #[test]
    fn a_tall_picture_gets_bars_on_the_sides() {
        let place = fit((1080, 1920), (1000, 1000));
        assert_eq!(place.y, 0.0);
        assert_eq!(place.height, 1000.0);
        assert_eq!(place.width, 563.0);
        assert_eq!(place.x, 218.0);
    }

    #[test]
    fn a_picture_grows_to_fill_a_bigger_surface() {
        let place = fit((1280, 720), (2560, 1440));
        assert_eq!(place.width, 2560.0);
        assert_eq!(place.height, 1440.0);
        assert_eq!((place.x, place.y), (0.0, 0.0));
    }

    /// Anything that can draw: a video card, or the system's software one (WARP on CI).
    fn painter(format: wgpu::TextureFormat) -> Painter {
        tauri::async_runtime::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("an adapter: WARP comes with Windows");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            Painter::new(device, queue, format)
        })
    }

    /// What the painter draws into a texture of `size`, as RGBA bytes.
    fn pixels(painter: &Painter, size: (u32, u32)) -> Vec<u8> {
        let target = painter.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        painter.draw(
            &target.create_view(&wgpu::TextureViewDescriptor::default()),
            size,
        );
        // Rows of a copy into a buffer are aligned to 256 bytes.
        let row = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = painter.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = painter
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        painter.queue().submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        painter
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped
            .chunks(row as usize)
            .flat_map(|r| r[..(size.0 * 4) as usize].to_vec())
            .collect()
    }

    fn filled(y: u8, u: u8, v: u8) -> I420Buffer {
        let mut picture = I420Buffer::new(16, 8);
        let (yp, up, vp) = picture.data_mut();
        yp.fill(y);
        up.fill(u);
        vp.fill(v);
        picture
    }

    fn pixel(all: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * width + x) * 4) as usize;
        [all[at], all[at + 1], all[at + 2]]
    }

    fn near(got: [u8; 3], want: [u8; 3]) -> bool {
        got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 3)
    }

    /// Colours as `argb_to_i420` writes them come back as they went in, and the picture
    /// keeps its shape: a 2:1 picture in a square leaves black above and below.
    #[test]
    fn colours_of_i420_come_out_as_rgb_with_bars_around() {
        let mut painter = painter(wgpu::TextureFormat::Rgba8Unorm);
        for (name, (y, u, v), want) in [
            ("red", (81, 90, 240), [255, 0, 0]),
            ("green", (145, 54, 34), [0, 255, 0]),
            ("blue", (41, 240, 110), [0, 0, 255]),
            ("white", (235, 128, 128), [255, 255, 255]),
            ("black", (16, 128, 128), [0, 0, 0]),
        ] {
            painter.upload(&filled(y, u, v));
            let all = pixels(&painter, (64, 64));
            let got = pixel(&all, 64, 32, 32);
            assert!(near(got, want), "{name}: {got:?}, want {want:?}");
            assert_eq!(pixel(&all, 64, 32, 4), [0, 0, 0], "{name}: the bar above");
            assert_eq!(pixel(&all, 64, 32, 60), [0, 0, 0], "{name}: the bar below");
        }
    }

    #[test]
    fn a_new_size_of_picture_makes_new_textures() {
        let mut painter = painter(wgpu::TextureFormat::Rgba8Unorm);
        painter.upload(&filled(235, 128, 128));
        let mut odd = I420Buffer::new(33, 17);
        let (y, u, v) = odd.data_mut();
        y.fill(81);
        u.fill(90);
        v.fill(240);
        painter.upload(&odd);
        let all = pixels(&painter, (66, 34));
        assert!(near(pixel(&all, 66, 33, 17), [255, 0, 0]));
    }
}
