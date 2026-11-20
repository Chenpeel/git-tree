use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};
use eframe::glow::{self, HasContext};
use glam::{Mat4, Vec3};

use crate::scene::Scene;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    from: [f32; 4],
    to: [f32; 4],
    color: [f32; 4],
    identity: [f32; 4],
}

pub struct RenderFrame {
    pub matrix: Mat4,
    pub eye: Vec3,
    pub pixel_scale: f32,
    pub selected: Option<usize>,
    pub hovered: Option<usize>,
    pub visible: Vec<(usize, f32)>,
    pub focus: Option<Arc<HashSet<usize>>>,
    pub matches: Arc<HashSet<usize>>,
    pub revision: u64,
}

struct Mesh {
    vao: glow::VertexArray,
    buffers: [glow::Buffer; 3],
    indices: i32,
}

impl Mesh {
    fn new(gl: &glow::Context, vertices: &[Vertex], indices: &[u32]) -> Result<Self> {
        // VAO 和三个缓冲在当前 eframe OpenGL 上下文内创建并统一释放.
        unsafe {
            let vao = gl.create_vertex_array().map_err(anyhow::Error::msg)?;
            let mut buffers = Vec::new();
            let result = (|| -> Result<Self> {
                for _ in 0..3 {
                    buffers.push(gl.create_buffer().map_err(anyhow::Error::msg)?);
                }
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffers[0]));
                gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    bytemuck::cast_slice(vertices),
                    glow::STATIC_DRAW,
                );
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffers[1]));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    bytemuck::cast_slice(indices),
                    glow::STATIC_DRAW,
                );
                for (location, size, offset) in [(0, 3, 0), (1, 3, 12), (2, 2, 24)] {
                    gl.enable_vertex_attrib_array(location);
                    gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, 32, offset);
                }
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffers[2]));
                for (location, offset) in [(3, 0), (4, 16), (5, 32), (6, 48)] {
                    gl.enable_vertex_attrib_array(location);
                    gl.vertex_attrib_pointer_f32(location, 4, glow::FLOAT, false, 64, offset);
                    gl.vertex_attrib_divisor(location, 1);
                }
                Ok(Self {
                    vao,
                    buffers: [buffers[0], buffers[1], buffers[2]],
                    indices: i32::try_from(indices.len())?,
                })
            })();
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            if result.is_err() {
                gl.delete_vertex_array(vao);
                for buffer in buffers {
                    gl.delete_buffer(buffer);
                }
            }
            result
        }
    }
    fn upload(&self, gl: &glow::Context, data: &[Instance]) {
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffers[2]));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(data),
                glow::DYNAMIC_DRAW,
            );
        }
    }
    fn draw(&self, gl: &glow::Context, count: usize) {
        if count == 0 {
            return;
        }
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements_instanced(
                glow::TRIANGLES,
                self.indices,
                glow::UNSIGNED_INT,
                0,
                count.min(i32::MAX as usize) as i32,
            );
        }
    }
    fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_vertex_array(self.vao);
            for buffer in self.buffers {
                gl.delete_buffer(buffer);
            }
        }
    }
}

pub struct Renderer {
    program: glow::Program,
    lines: Mesh,
    nodes: Mesh,
    line_count: usize,
    revision: Option<u64>,
    matrix: glow::UniformLocation,
    eye: glow::UniformLocation,
    pixel_scale: glow::UniformLocation,
    shape: glow::UniformLocation,
    selected: glow::UniformLocation,
    hovered: glow::UniformLocation,
    destroyed: bool,
}

impl Renderer {
    pub fn new(gl: &glow::Context) -> Result<Self> {
        let program = make_program(gl)?;
        let mut meshes = Vec::new();
        let result = (|| -> Result<Self> {
            let uniform = |name| {
                unsafe { gl.get_uniform_location(program, name) }
                    .with_context(|| format!("Git 3D shader uniform missing: {name}"))
            };
            let matrix = uniform("u_matrix")?;
            let eye = uniform("u_eye")?;
            let pixel_scale = uniform("u_pixel_scale")?;
            let shape = uniform("u_shape")?;
            let selected = uniform("u_selected")?;
            let hovered = uniform("u_hovered")?;
            for geometry in [line_mesh(), sphere_mesh()] {
                meshes.push(Mesh::new(gl, &geometry.0, &geometry.1)?);
            }
            let nodes = meshes.pop().expect("Node mesh created");
            let lines = meshes.pop().expect("Line mesh created");
            Ok(Self {
                program,
                lines,
                nodes,
                line_count: 0,
                revision: None,
                matrix,
                eye,
                pixel_scale,
                shape,
                selected,
                hovered,
                destroyed: false,
            })
        })();
        if result.is_err() {
            for mesh in meshes {
                mesh.destroy(gl);
            }
            unsafe {
                gl.delete_program(program);
            }
        }
        result
    }

    pub fn paint(&mut self, gl: &glow::Context, scene: &Scene, frame: &RenderFrame) {
        if self.destroyed {
            return;
        }
        if self.revision != Some(frame.revision) {
            let lines: Vec<_> = scene
                .branches
                .iter()
                .map(|edge| {
                    let focused = frame.focus.as_ref().is_none_or(|path| {
                        path.contains(&edge.child) && path.contains(&edge.parent)
                    });
                    Instance {
                        from: [edge.from.x, edge.from.y, edge.from.z, edge.radius],
                        to: [edge.to.x, edge.to.y, edge.to.z, 0.0],
                        color: [
                            edge.color[0],
                            edge.color[1],
                            edge.color[2],
                            if !focused {
                                0.14
                            } else if edge.merge {
                                0.48
                            } else if edge.trunk {
                                0.92
                            } else {
                                0.76
                            },
                        ],
                        identity: [-1.0; 4],
                    }
                })
                .collect();
            self.lines.upload(gl, &lines);
            self.line_count = lines.len();
            self.revision = Some(frame.revision);
        }
        let nodes: Vec<_> = frame
            .visible
            .iter()
            .map(|&(index, radius)| {
                let node = &scene.nodes[index];
                let color = if frame.matches.contains(&index) {
                    [1.0, 0.81, 0.35]
                } else {
                    node.color
                };
                let focus = if frame
                    .focus
                    .as_ref()
                    .is_none_or(|path| path.contains(&index))
                {
                    1.0
                } else {
                    0.22
                };
                Instance {
                    from: [node.position.x, node.position.y, node.position.z, radius],
                    to: [0.0; 4],
                    color: [color[0], color[1], color[2], focus],
                    identity: [index as f32, 0.0, 0.0, 0.0],
                }
            })
            .collect();
        self.nodes.upload(gl, &nodes);
        // 清除当前 egui 裁剪区域内的深度, 保留界面与星空背景.
        unsafe {
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(Some(&self.matrix), false, &frame.matrix.to_cols_array());
            gl.uniform_3_f32(Some(&self.eye), frame.eye.x, frame.eye.y, frame.eye.z);
            gl.uniform_1_f32(Some(&self.pixel_scale), frame.pixel_scale);
            gl.uniform_1_f32(
                Some(&self.selected),
                frame.selected.map_or(-2.0, |index| index as f32),
            );
            gl.uniform_1_f32(
                Some(&self.hovered),
                frame.hovered.map_or(-2.0, |index| index as f32),
            );
            gl.uniform_1_i32(Some(&self.shape), 0);
        }
        self.lines.draw(gl, self.line_count);
        unsafe {
            gl.uniform_1_i32(Some(&self.shape), 1);
        }
        self.nodes.draw(gl, nodes.len());
        // egui 不替自定义回调恢复深度测试和写入状态.
        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        self.lines.destroy(gl);
        self.nodes.destroy(gl);
        unsafe {
            gl.delete_program(self.program);
        }
    }
}

fn make_program(gl: &glow::Context) -> Result<glow::Program> {
    unsafe {
        let program = gl.create_program().map_err(anyhow::Error::msg)?;
        let mut shaders = Vec::new();
        let result = (|| -> Result<()> {
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ] {
                let shader = gl.create_shader(kind).map_err(anyhow::Error::msg)?;
                shaders.push((shader, false));
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    bail!(
                        "Git 3D shader compilation failed: {}",
                        gl.get_shader_info_log(shader)
                    );
                }
                gl.attach_shader(program, shader);
                shaders.last_mut().expect("Shader created").1 = true;
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                bail!(
                    "Git 3D shader link failed: {}",
                    gl.get_program_info_log(program)
                );
            }
            Ok(())
        })();
        for (shader, attached) in shaders {
            if attached {
                gl.detach_shader(program, shader);
            }
            gl.delete_shader(shader);
        }
        if let Err(error) = result {
            gl.delete_program(program);
            return Err(error);
        }
        Ok(program)
    }
}

fn line_mesh() -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for ring in 0..=1 {
        for side in 0..=8 {
            let angle = side as f32 / 8.0 * std::f32::consts::TAU;
            vertices.push(Vertex {
                position: [angle.cos(), ring as f32, angle.sin()],
                normal: [angle.cos(), 0.0, angle.sin()],
                uv: [0.0; 2],
            });
        }
    }
    for side in 0..8_u32 {
        let b = side + 9;
        indices.extend([side, b, side + 1, side + 1, b, b + 1]);
    }
    (vertices, indices)
}
fn sphere_mesh() -> (Vec<Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for latitude in 0..=8 {
        for longitude in 0..=12 {
            let t = latitude as f32 / 8.0 * std::f32::consts::PI;
            let p = longitude as f32 / 12.0 * std::f32::consts::TAU;
            let point = [t.sin() * p.cos(), t.cos(), t.sin() * p.sin()];
            vertices.push(Vertex {
                position: point,
                normal: point,
                uv: [0.0; 2],
            });
        }
    }
    for latitude in 0..8 {
        for longitude in 0..12 {
            let a = latitude * 13 + longitude;
            let b = a + 13;
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (vertices, indices)
}
const VERTEX_SHADER: &str = r#"#version 330 core
layout(location=0) in vec3 a_position;
layout(location=1) in vec3 a_normal;
layout(location=3) in vec4 i_from;
layout(location=4) in vec4 i_to;
layout(location=5) in vec4 i_color;
layout(location=6) in vec4 i_identity;
uniform mat4 u_matrix;
uniform vec3 u_eye;
uniform float u_pixel_scale;
uniform int u_shape;
uniform float u_selected;
uniform float u_hovered;
out vec3 v_world;
out vec3 v_normal;
out vec3 v_color;
out float v_focus;
out float v_active;
void main(){
    float selected=float(u_shape==1&&abs(i_identity.x-u_selected)<0.25);
    float hovered=float(u_shape==1&&abs(i_identity.x-u_hovered)<0.25);
    v_active=max(selected,hovered*0.65);v_focus=mix(i_color.a,1.0,v_active);
    v_color=mix(i_color.rgb,vec3(1.0,0.81,0.4),selected);
    if(u_shape==0){
        vec3 delta=i_to.xyz-i_from.xyz;float segment_length=max(length(delta),0.00001);vec3 axis=delta/segment_length;
        vec3 helper=abs(axis.y)>0.95?vec3(1,0,0):vec3(0,1,0);
        vec3 right=normalize(cross(axis,helper));vec3 forward=cross(right,axis);
        float along=clamp(dot(u_eye-i_from.xyz,delta)/max(dot(delta,delta),1e-12),0.0,1.0);
        float radius=max(i_from.w,distance(u_eye,i_from.xyz+delta*along)*u_pixel_scale*0.5);
        v_world=i_from.xyz+delta*a_position.y+radius*(right*a_position.x+forward*a_position.z);
        v_normal=normalize(right*a_normal.x+forward*a_normal.z);
    }else{v_world=i_from.xyz+a_position*i_from.w;v_normal=a_normal;}
    gl_Position=u_matrix*vec4(v_world,1.0);
}
"#;
const FRAGMENT_SHADER: &str = r#"#version 330 core
in vec3 v_world;
in vec3 v_normal;
in vec3 v_color;
in float v_focus;
in float v_active;
uniform vec3 u_eye;
uniform int u_shape;
out vec4 out_color;
void main(){
    vec3 normal=normalize(v_normal);vec3 view=normalize(u_eye-v_world);
    vec3 key=normalize(vec3(-0.45,0.8,0.65));vec3 fill=normalize(vec3(0.6,0.2,-0.6));
    float diffuse=max(dot(normal,key),0.0);float back=max(dot(normal,fill),0.0);
    float rim=pow(1.0-max(dot(normal,view),0.0),3.0);
    float emission=(u_shape==1?0.16:0.05)+v_active*0.45;
    float specular=pow(max(dot(normal,normalize(key+view)),0.0),32.0)*0.22;
    vec3 color=v_color*(0.28+diffuse*0.65+back*0.18+emission)+vec3(0.25,0.4,0.42)*rim*0.24+specular;
    color=mix(vec3(0.035,0.06,0.075),color,v_focus);out_color=vec4(color,1.0);
}
"#;
