@group(0) @binding(0) var<storage, read_write> states : array<u32>;
@group(0) @binding(1) var<storage, read> inputs : array<vec2<u32>>;
@group(0) @binding(2) var<storage, read> modes : array<u32>;

const ERROR : u32 = 987654321u;
const NONE : u32 = 123456789u;
const MODE_IDENTITY : u32 = 0u;
const MODE_NOT : u32 = 1u;
const MODE_AND : u32 = 2u;
const MODE_OR : u32 = 3u;
const MODE_XOR : u32 = 4u;
const MODE_ALWAYSA : u32 = 5u;
const MODE_ALWAYSB : u32 = 6u;
const MODE_ALWAYSLO : u32 = 7u;
const MODE_ALWAYSHI : u32 = 8u;

fn eval(mode: u32, in_a: u32, in_b: u32) -> u32 {
    switch mode {
        case 0u, 5u: {
            return in_a;
        }
        case 6u: {
            return in_b;
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
        case 7u: {
            return 0u;
        }
        case 8u: {
            return 1u;
        }
        default: {
            return ERROR;
        }
    }
}


@compute
@workgroup_size(1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // if global_id.x >= arrayLength(&modes) { return; }
    let in_a: u32 = states[ inputs[global_id.x].x ];
    let in_b = 0u;
    if inputs[global_id.x].y == NONE {
        states[global_id.x] = eval(modes[global_id.x], in_a, 0u);
    } else {
        states[global_id.x] = eval(modes[global_id.x], in_a, states[ inputs[global_id.x].y ]);
    };
}