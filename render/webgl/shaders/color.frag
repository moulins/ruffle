#version 100
precision mediump float;

uniform mat4 view_matrix;
uniform mat4 world_matrix;
uniform vec4 mult_color;
uniform vec4 add_color;

varying vec4 frag_color;

void main() {
    gl_FragColor = frag_color;
}
