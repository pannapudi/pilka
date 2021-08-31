
#version 460

layout(location = 0) out vec2 out_uv;

layout(std430, push_constant) uniform PushConstant {
	vec3 pos;
	float time;
	vec2 resolution;
	vec2 mouse;
} pc;

void main() {
    out_uv = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(out_uv * 2.0f + -1.0f, 0.0, 1.0);
    out_uv = (out_uv + -0.5) * 2.0 / vec2(pc.resolution.y / pc.resolution.x, 1);
}
