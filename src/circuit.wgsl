@group(0) @binding(0) var<storage, read> states : array<u32>;
@group(0) @binding(1) var<storage, read_write> states_out : array<u32>;
@group(0) @binding(2) var<storage, read> inputs : array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> modes : array<u32>;

const NONE : u32 = 0xFFFFFFFFu;
const MODE_IDENTITY : u32 = 0u;
const MODE_NOT : u32 = 1u;
const MODE_AND : u32 = 2u;
const MODE_OR : u32 = 3u;
const MODE_XOR : u32 = 4u;

fn eval(mode: u32, in_a: u32, in_b: u32) -> u32 {
    switch mode {
        case 0u: {
            return in_a;
        }
        case 1u: {
            if in_a == 0u {
                return 1u;
            } else {
                return 0u;
            }
        }
        case 2u: {
            return in_a & in_b;
        }
        case 3u: {
            return in_a | in_b;
        }
        case 4u: {
            return in_a ^ in_b;
        }
        default: {
            return NONE;
        }
    }
}

@compute
@workgroup_size(1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let in_a: u32 = states[ inputs[global_id.x].x ];
    if inputs[global_id.x].y == NONE {
        states_out[global_id.x] = eval(modes[global_id.x], in_a, 0u);
    } else {
        states_out[global_id.x] = eval(modes[global_id.x], in_a, states[ inputs[global_id.x].y ]);
    };
}