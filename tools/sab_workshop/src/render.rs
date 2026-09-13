//! wgpu renderer: GPU-skinned mesh (skinning in the vertex shader) + ground grid + depth,
//! all drawn under an egui overlay. wgpu 0.20.

use std::sync::Arc;

use glam::Mat4;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::dtex::CpuTexture;
use crate::formats::{self, Smsh, SubMesh};
use crate::gui::Gui;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    joints: [u16; 4],
    weights: [f32; 4],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GridVertex {
    pos: [f32; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    surface_format: wgpu::TextureFormat,

    depth_view: wgpu::TextureView,

    camera_buf: wgpu::Buffer,
    joint_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,

    mesh_pipeline: wgpu::RenderPipeline,
    vbo: wgpu::Buffer,
    ibo: wgpu::Buffer,
    index_count: u32,

    // Per-submesh (leaf-range cover) draw list + its bound textures. A submesh always has ONE
    // BindGroup — slot views that are `None` bind the shared fallback (white for diffuse, flat
    // normal for normal). Same order as `submeshes`.
    submeshes: Vec<SubMesh>,
    submesh_diffuse: Vec<Option<wgpu::TextureView>>,
    submesh_normal: Vec<Option<wgpu::TextureView>>,
    submesh_bg: Vec<wgpu::BindGroup>,
    tex_bgl: wgpu::BindGroupLayout,
    scene_bgl: wgpu::BindGroupLayout, // kept so the joint buffer can be resized on a mesh swap
    sampler: wgpu::Sampler,
    /// 1x1 white — the fallback diffuse view (unbound submeshes render as plain lit geometry).
    white_view: wgpu::TextureView,
    /// 1x1 flat-normal `(128, 128, 255, 255)` — the fallback normal view (a "no bump" tangent normal
    /// pointing straight out, so N·L collapses to the vertex-space lambert).
    flat_normal_view: wgpu::TextureView,
    /// Shorthand BG bound when `show_textures` is off — everything renders as untextured lit.
    fallback_bg: wgpu::BindGroup,

    grid_pipeline: wgpu::RenderPipeline,
    grid_vbo: wgpu::Buffer,
    grid_count: u32,

    joint_count: usize,
    pub show_grid: bool,
    /// Draw the 3D scene at all. False on the editor pages, which are not a 3D view.
    pub draw_scene: bool,
    pub show_textures: bool,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, mesh: &Smsh, joint_count: usize) -> Result<Renderer, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no suitable GPU adapter")?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("sab_workshop device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("request_device: {e}"))?;

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface unsupported by adapter")?;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        surface.configure(&device, &config);
        let surface_format = config.format;

        let depth_view = make_depth(&device, &config);

        // --- uniforms / storage ---
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Joint matrices (skinning). Start at bind pose (identity == bind if inv_bind consistent).
        let jc = joint_count.max(1);
        let ident: Vec<[[f32; 4]; 4]> = vec![Mat4::IDENTITY.to_cols_array_2d(); jc];
        let joint_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("joints"),
            contents: bytemuck::cast_slice(&ident),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: joint_buf.as_entire_binding() },
            ],
        });

        // --- per-material texture bind group (group 1): diffuse + normal + one shared sampler ---
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tex bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // Normal map — sampled with the SAME sampler as diffuse. This is only correct because
                // Saboteur's `_NM` textures are tangent-space DXT1 (see normalmap.rs guard test); a
                // grayscale-relief convention would need re-derivation before upload.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mesh sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Shared 1x1 fallbacks: white for the diffuse slot, flat OpenGL-tangent normal for the normal
        // slot (encodes Z ≈ +1 → the fragment TBN's N is used unchanged, so lighting reduces to the
        // vertex-normal lambert).
        let white_view = make_tex_view(&device, &queue, 1, 1, &[255, 255, 255, 255]);
        let flat_normal_view =
            make_tex_view(&device, &queue, 1, 1, &crate::normalmap::FLAT_NORMAL_RGBA);
        let fallback_bg = make_submesh_bg(&device, &tex_bgl, &sampler, &white_view, &flat_normal_view);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene pl"),
            bind_group_layouts: &[&bgl, &tex_bgl],
            push_constant_ranges: &[],
        });
        // The grid pipeline uses group 0 only, so it needs its own layout (no texture group).
        let grid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grid pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        // --- mesh pipeline ---
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh shader"),
            source: wgpu::ShaderSource::Wgsl(MESH_WGSL.into()),
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32x3, 2 => Uint16x4, 3 => Float32x4, 4 => Float32x2
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // --- mesh buffers ---
        let verts = build_vertices(mesh);
        let vbo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vbo"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh ibo"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let index_count = mesh.indices.len() as u32;

        // Non-overlapping per-material draw list; every submesh starts untextured (white + flat).
        let submeshes = formats::submesh_cover(&mesh.prims, index_count);
        let submesh_diffuse: Vec<Option<wgpu::TextureView>> = (0..submeshes.len()).map(|_| None).collect();
        let submesh_normal: Vec<Option<wgpu::TextureView>> = (0..submeshes.len()).map(|_| None).collect();
        let submesh_bg: Vec<wgpu::BindGroup> = (0..submeshes.len())
            .map(|_| make_submesh_bg(&device, &tex_bgl, &sampler, &white_view, &flat_normal_view))
            .collect();

        // --- grid pipeline + geometry ---
        let grid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grid shader"),
            source: wgpu::ShaderSource::Wgsl(GRID_WGSL.into()),
        });
        let grid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grid pipeline"),
            layout: Some(&grid_layout),
            vertex: wgpu::VertexState {
                module: &grid_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GridVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &grid_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let grid_verts = build_grid(10, 0.5);
        let grid_count = grid_verts.len() as u32;
        let grid_vbo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grid vbo"),
            contents: bytemuck::cast_slice(&grid_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            surface_format,
            depth_view,
            camera_buf,
            joint_buf,
            bind_group,
            mesh_pipeline,
            vbo,
            ibo,
            index_count,
            submeshes,
            submesh_diffuse,
            submesh_normal,
            submesh_bg,
            tex_bgl,
            scene_bgl: bgl,
            sampler,
            white_view,
            flat_normal_view,
            fallback_bg,
            grid_pipeline,
            grid_vbo,
            grid_count,
            joint_count: jc,
            show_grid: true,
            draw_scene: true,
            show_textures: true,
        })
    }

    pub fn device(&self) -> &wgpu::Device { &self.device }
    pub fn surface_format(&self) -> wgpu::TextureFormat { self.surface_format }
    pub fn size(&self) -> [u32; 2] { [self.config.width, self.config.height] }
    pub fn aspect(&self) -> f32 { self.config.width as f32 / self.config.height.max(1) as f32 }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = make_depth(&self.device, &self.config);
    }

    pub fn update_camera(&self, view_proj: Mat4) {
        let u = CameraUniform { view_proj: view_proj.to_cols_array_2d() };
        self.queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&u));
    }

    pub fn update_joints(&self, mats: &[Mat4]) {
        let mut data: Vec<[[f32; 4]; 4]> = Vec::with_capacity(self.joint_count);
        for i in 0..self.joint_count {
            data.push(mats.get(i).copied().unwrap_or(Mat4::IDENTITY).to_cols_array_2d());
        }
        self.queue.write_buffer(&self.joint_buf, 0, bytemuck::cast_slice(&data));
    }

    /// The per-material draw list (leaf-range cover). The resolver walks this to map each submesh's
    /// candidate material hashes to a diffuse DTEX.
    pub fn submeshes(&self) -> &[SubMesh] {
        &self.submeshes
    }

    /// Swap in a different model at runtime (the navigator's click-to-load). Rebuilds the vertex /
    /// index buffers and the per-material draw list, drops every bound texture, and — when the new
    /// rig has a different bone count — reallocates the joint storage buffer (which forces the scene
    /// bind group to be rebuilt, since it points at that buffer).
    pub fn set_mesh(&mut self, mesh: &Smsh, bone_count: usize) {
        let verts = build_vertices(mesh);
        self.vbo = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vbo"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.ibo = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh ibo"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.index_count = mesh.indices.len() as u32;
        self.submeshes = formats::submesh_cover(&mesh.prims, self.index_count);
        self.submesh_diffuse = (0..self.submeshes.len()).map(|_| None).collect();
        self.submesh_normal = (0..self.submeshes.len()).map(|_| None).collect();
        self.submesh_bg = (0..self.submeshes.len())
            .map(|_| {
                make_submesh_bg(
                    &self.device,
                    &self.tex_bgl,
                    &self.sampler,
                    &self.white_view,
                    &self.flat_normal_view,
                )
            })
            .collect();

        let jc = bone_count.max(1);
        if jc != self.joint_count {
            self.joint_count = jc;
            let ident: Vec<[[f32; 4]; 4]> = vec![Mat4::IDENTITY.to_cols_array_2d(); jc];
            self.joint_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("joints"),
                contents: bytemuck::cast_slice(&ident),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
            self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene bg"),
                layout: &self.scene_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.camera_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.joint_buf.as_entire_binding() },
                ],
            });
        }
    }

    /// Upload `tex` as submesh `i`'s diffuse and rebuild that submesh's bind group.
    pub fn set_submesh_texture(&mut self, i: usize, tex: &CpuTexture) {
        if i >= self.submeshes.len() {
            return;
        }
        self.submesh_diffuse[i] =
            Some(make_tex_view(&self.device, &self.queue, tex.width.max(1), tex.height.max(1), &tex.rgba));
        self.rebuild_submesh_bg(i);
    }

    /// Upload `tex` as submesh `i`'s tangent-space normal map. Rebuilds the bind group. Saboteur's
    /// `_NM` records are already tangent-space DXT1 (see the guard test in `normalmap.rs`), so we
    /// upload the decoded bytes as-is with no signed re-encoding.
    pub fn set_submesh_normal(&mut self, i: usize, tex: &CpuTexture) {
        if i >= self.submeshes.len() {
            return;
        }
        self.submesh_normal[i] =
            Some(make_tex_view(&self.device, &self.queue, tex.width.max(1), tex.height.max(1), &tex.rgba));
        self.rebuild_submesh_bg(i);
    }

    /// Unassign submesh `i`'s diffuse (falls back to white). The normal slot is unchanged.
    pub fn clear_submesh_texture(&mut self, i: usize) {
        if i >= self.submeshes.len() {
            return;
        }
        self.submesh_diffuse[i] = None;
        self.rebuild_submesh_bg(i);
    }

    /// Unassign submesh `i`'s normal (falls back to the flat OpenGL-tangent normal).
    pub fn clear_submesh_normal(&mut self, i: usize) {
        if i >= self.submeshes.len() {
            return;
        }
        self.submesh_normal[i] = None;
        self.rebuild_submesh_bg(i);
    }

    /// Drop every resolved diffuse (back to the flat white look). Normals are unchanged.
    pub fn clear_textures(&mut self) {
        for i in 0..self.submeshes.len() {
            self.submesh_diffuse[i] = None;
            self.rebuild_submesh_bg(i);
        }
    }

    fn rebuild_submesh_bg(&mut self, i: usize) {
        let d = self.submesh_diffuse[i].as_ref().unwrap_or(&self.white_view);
        let n = self.submesh_normal[i].as_ref().unwrap_or(&self.flat_normal_view);
        self.submesh_bg[i] = make_submesh_bg(&self.device, &self.tex_bgl, &self.sampler, d, n);
    }

    /// Draw one frame: 3D scene (grid + skinned mesh) then the egui overlay.
    pub fn render(&mut self, gui: &mut Gui) -> Result<(), wgpu::SurfaceError> {
        let output = match self.surface.get_current_texture() {
            Ok(o) => o,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.surface.get_current_texture()?
            }
            Err(e) => return Err(e),
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.08, g: 0.09, b: 0.11, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // The editor pages are not a 3D view: drawing the character behind them costs a full
            // scene pass every frame AND shows through anywhere egui does not paint, which reads as
            // a rendering bug. The clear still runs, so the ground under the panels is the theme's.
            if self.draw_scene {
                pass.set_bind_group(0, &self.bind_group, &[]);
                if self.show_grid {
                    pass.set_pipeline(&self.grid_pipeline);
                    pass.set_vertex_buffer(0, self.grid_vbo.slice(..));
                    pass.draw(0..self.grid_count, 0..1);
                }
                pass.set_pipeline(&self.mesh_pipeline);
                pass.set_vertex_buffer(0, self.vbo.slice(..));
                pass.set_index_buffer(self.ibo.slice(..), wgpu::IndexFormat::Uint32);
                // One draw per submesh, each with its own diffuse (or the white fallback). When
                // textures are toggled off, everything binds white → the original flat-lit look.
                for (i, sm) in self.submeshes.iter().enumerate() {
                    let bg = if self.show_textures { &self.submesh_bg[i] } else { &self.fallback_bg };
                    pass.set_bind_group(1, bg, &[]);
                    let end = sm.index_start + sm.index_count;
                    pass.draw_indexed(sm.index_start..end, 0, 0..1);
                }
            }
        }
        gui.paint(&self.device, &self.queue, &mut encoder, &view, self.size());
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }
}

/// Interleave an `Smsh` into the renderer's vertex layout.
fn build_vertices(mesh: &Smsh) -> Vec<Vertex> {
    (0..mesh.positions.len())
        .map(|i| Vertex {
            pos: mesh.positions[i],
            normal: *mesh.normals.get(i).unwrap_or(&[0.0, 1.0, 0.0]),
            joints: *mesh.joints.get(i).unwrap_or(&[0, 0, 0, 0]),
            weights: *mesh.weights.get(i).unwrap_or(&[1.0, 0.0, 0.0, 0.0]),
            uv: *mesh.uvs.get(i).unwrap_or(&[0.0, 0.0]),
        })
        .collect()
}

/// Upload `rgba` (w*h*4, straight-alpha) as an `Rgba8Unorm` texture and return its view. We keep the
/// stored bytes as-is (no sRGB decode) so the preview matches a raw DDS view and stays consistent
/// with the flat/grid passes — and, for normal maps, so the tangent-space bytes are read verbatim.
fn make_tex_view(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Guard against a short/oversized buffer so a malformed decode can't panic write_texture.
    let need = (w * h * 4) as usize;
    let mut data = rgba.to_vec();
    data.resize(need, 0);
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        size,
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Build one submesh's group-1 bind group over the shared `tex_bgl`: (diffuse, sampler, normal).
fn make_submesh_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    diffuse: &wgpu::TextureView,
    normal: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("submesh bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(diffuse) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(normal) },
        ],
    })
}

fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: config.width.max(1),
            height: config.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn build_grid(half: i32, step: f32) -> Vec<GridVertex> {
    let mut v = Vec::new();
    let ext = half as f32 * step;
    let line = |v: &mut Vec<GridVertex>, a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        v.push(GridVertex { pos: a, color: c });
        v.push(GridVertex { pos: b, color: c });
    };
    for i in -half..=half {
        let p = i as f32 * step;
        let axis = i == 0;
        let cz = if axis { [0.45, 0.30, 0.30] } else { [0.22, 0.22, 0.26] };
        let cx = if axis { [0.30, 0.30, 0.45] } else { [0.22, 0.22, 0.26] };
        line(&mut v, [p, 0.0, -ext], [p, 0.0, ext], cz); // lines parallel to Z
        line(&mut v, [-ext, 0.0, p], [ext, 0.0, p], cx); // lines parallel to X
    }
    v
}

const MESH_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> joints: array<mat4x4<f32>>;
@group(1) @binding(0) var diffuse: texture_2d<f32>;
@group(1) @binding(1) var mesh_samp: sampler;
@group(1) @binding(2) var normal_tex: texture_2d<f32>;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) joints: vec4<u32>,
    @location(3) weights: vec4<f32>,
    @location(4) uv: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // Fragment-space TBN reconstruction needs the world-space position, so pass it through.
    @location(2) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let wsum = in.weights.x + in.weights.y + in.weights.z + in.weights.w;
    var world_pos: vec4<f32>;
    var world_nrm: vec3<f32>;
    if (wsum < 0.0001) {
        // Unweighted vertex: leave it in bind space.
        world_pos = vec4<f32>(in.pos, 1.0);
        world_nrm = in.normal;
    } else {
        let m = joints[in.joints.x] * in.weights.x
              + joints[in.joints.y] * in.weights.y
              + joints[in.joints.z] * in.weights.z
              + joints[in.joints.w] * in.weights.w;
        world_pos = m * vec4<f32>(in.pos, 1.0);
        world_nrm = (m * vec4<f32>(in.normal, 0.0)).xyz;
    }
    var out: VsOut;
    out.clip = camera.view_proj * world_pos;
    out.normal = world_nrm;
    out.uv = in.uv;
    out.world_pos = world_pos.xyz;
    return out;
}

// Christian Schüler's cotangent-frame reconstruction — recovers a per-fragment TBN from the world
// position and UV derivatives, so we don't need per-vertex tangents (Saboteur meshes don't carry
// them). Reference: http://www.thetenthplanet.de/archives/1180.
fn cotangent_frame(n: vec3<f32>, p: vec3<f32>, uv: vec2<f32>) -> mat3x3<f32> {
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let invmax = inverseSqrt(max(dot(t, t), dot(b, b)));
    return mat3x3<f32>(t * invmax, b * invmax, n);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(diffuse, mesh_samp, in.uv);
    let vn = normalize(in.normal);

    // Sample tangent-space normal (Saboteur's `_NM` is OpenGL-convention DXT1: R=X, G=+Y, B=+Z).
    // Where the submesh has no normal map bound, the fallback is the flat (0.5, 0.5, 1.0) texel, so
    // `n_tan` = (0, 0, 1) and the TBN transform leaves `vn` unchanged — the shader degrades to the
    // old vertex-normal lambert with no branching.
    let n_tan = textureSample(normal_tex, mesh_samp, in.uv).xyz * 2.0 - 1.0;
    let tbn = cotangent_frame(vn, in.world_pos, in.uv);
    let n = normalize(tbn * n_tan);

    // Key light from above-front-right; the same direction the pre-normal-map shader used, so a
    // model with no `_NM` reads identically to before on the diffuse term.
    let l = normalize(vec3<f32>(0.4, 0.85, 0.55));
    let d = abs(dot(n, l));
    let diffuse_shade = 0.18 + 0.82 * d;

    // Blinn-Phong specular against the sampled normal — the term that MAKES normal maps visible
    // (a plain lambert barely shows tangent-space perturbation). Held to a tight, dim highlight
    // because there is no spec-map modulation yet — the game's slot-1 texture would per-pixel-mask
    // which surfaces are shiny (skin, leather) vs matte (cloth, hair); as a flat constant it turns
    // cloth into PVC. See the note above `fs_main` — wiring the spec map is the proper fix.
    let view_dir = normalize(cross(dpdy(in.world_pos), dpdx(in.world_pos)));
    let half_v = normalize(l + view_dir);
    let n_dot_h = max(dot(n, half_v), 0.0);
    let spec = pow(n_dot_h, 64.0) * 0.15;

    let color = base.rgb * diffuse_shade + vec3<f32>(spec);
    return vec4<f32>(color, 1.0);
}
"#;

const GRID_WGSL: &str = r#"
struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec3<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;
