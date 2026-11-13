//! Puts the GPU probe set to the `wasi:nn` backend the runner hosts, which evaluates it on the CPU.
//!
//! The probes are spelled as an ONNX graph, one operator per operation, and ONNX Runtime's own kernels answer them: no
//! GPU and no browser is involved, so the signature is that of the runtime's arithmetic on this machine. It is checked
//! against the one recorded for the machine, and printed, which is how a machine's signature comes to be recorded.
#![cfg(test)]

use et_gpu_probe::{Answer, INPUTS, PROBE_COUNT, Signature, WASI_NN, shaders};
use wasmtime_wasi_nn::backend::NamedTensor;
use wasmtime_wasi_nn::wit::{ExecutionTarget, TensorType};
use wasmtime_wasi_nn::{ExecutionContext, Tensor};

/// The name each of [`INPUTS`] has in the graph, in order, which is the name the probe shaders give it.
const INPUT_NAMES: [&str; 8] = [
    "ulpBase",
    "negatedSquare",
    "big",
    "one",
    "angle",
    "three",
    "tiny",
    "oneHalf",
];

/// Each operator of the probe graph that takes no attribute: its name, its inputs, and the tensor it produces.
///
/// Every tensor is a 1x1 matrix, so that the operators with a matrix kernel behind them take part. `Gemm` is the one
/// operator that multiplies and adds in a single kernel, which is what `fma` asks for, and `MatMul` of a row by a
/// column is the dot product. ONNX has no two-argument arctangent, so `atan2` is the arctangent of the quotient, which
/// is the same value while the divisor is positive, as it is here.
const OPERATORS: [(&str, &[&str], &str); 19] = [
    ("Mul", &["ulpBase", "ulpBase"], "square"),
    ("Add", &["square", "negatedSquare"], "multiply_add"),
    ("Gemm", &["ulpBase", "ulpBase", "negatedSquare"], "fma"),
    ("ReduceSum", &["terms"], "sum"),
    ("Sub", &["sum", "big"], "swamped_sum"),
    ("Div", &["one", "three"], "divide"),
    ("Mul", &["tiny", "oneHalf"], "subnormal_product"),
    ("Sqrt", &["three"], "sqrt"),
    ("Reciprocal", &["sqrt"], "inverse_sqrt"),
    ("Sin", &["angle"], "sin"),
    ("Cos", &["angle"], "cos"),
    ("Tan", &["angle"], "tan"),
    ("Exp", &["angle"], "exp"),
    ("Log", &["three"], "log"),
    ("Pow", &["three", "angle"], "pow"),
    ("Div", &["angle", "three"], "slope"),
    ("Atan", &["slope"], "atan2"),
    ("Tanh", &["angle"], "tanh"),
    ("MatMul", &["row", "column"], "dot"),
];

/// A protobuf message being written: the bytes of the fields given to it so far.
#[derive(Default)]
struct Message(Vec<u8>);

impl Message {
    /// Append `value` as a base-128 varint, low seven bits first.
    fn push_varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(u8::try_from(value & 0x7f).unwrap() | 0x80);
            value >>= 7_u32;
        }
        self.0.push(u8::try_from(value).unwrap());
    }

    /// This message with the integer `value` as its field numbered `field`.
    fn int(mut self, field: u64, value: u64) -> Self {
        self.push_varint(field << 3_u32);
        self.push_varint(value);
        self
    }

    /// This message with `payload` as its length-delimited field numbered `field`.
    fn bytes(mut self, field: u64, payload: &[u8]) -> Self {
        self.push_varint((field << 3_u32) | 2);
        self.push_varint(u64::try_from(payload.len()).unwrap());
        self.0.extend_from_slice(payload);
        self
    }

    /// This message with `text` as its string field numbered `field`.
    fn text(self, field: u64, text: &str) -> Self {
        self.bytes(field, text.as_bytes())
    }

    /// This message with `inner` as its nested message field numbered `field`.
    fn message(self, field: u64, inner: &Self) -> Self {
        self.bytes(field, &inner.0)
    }
}

/// An ONNX `NodeProto` applying `operator` to `inputs` and naming what it produces `outputs`.
fn node(operator: &str, inputs: &[&str], outputs: &[&str]) -> Message {
    let named = inputs
        .iter()
        .fold(Message::default(), |message, input| message.text(1, input));
    outputs
        .iter()
        .fold(named, |message, output| message.text(2, output))
        .text(4, operator)
}

/// `node` with the integer attribute `axis`, which `Split` and `Concat` work along.
fn along(node: Message, axis: u64) -> Message {
    node.message(5, &Message::default().text(1, "axis").int(3, axis).int(20, 2))
}

/// An ONNX `ValueInfoProto` for the `f32` tensor `name` of one row and `width` columns.
fn row_of(name: &str, width: u64) -> Message {
    let dimension = |size| Message::default().int(1, size);
    let shape = Message::default()
        .message(1, &dimension(1))
        .message(1, &dimension(width));
    let tensor = Message::default().int(1, 1).message(2, &shape);
    Message::default()
        .text(1, name)
        .message(2, &Message::default().message(1, &tensor))
}

/// The probe set as an ONNX model: [`INPUTS`] in as one row, every probe's answer out as another.
#[expect(
    clippy::single_call_fn,
    reason = "the graph's construction, kept apart from the run that answers it"
)]
fn probe_model() -> Vec<u8> {
    let probes = shaders();
    let answers: Vec<&str> = probes.iter().map(|probe| probe.0).collect();
    let graph = [
        along(node("Split", &["probeIn"], &INPUT_NAMES), 1),
        node("Neg", &["big"], &["negatedBig"]),
        along(node("Concat", &["big", "one", "one", "one", "one"], &["terms"]), 1),
        along(node("Concat", &["big", "one", "one", "negatedBig"], &["row"]), 1),
        along(node("Concat", &["one", "one", "one", "one"], &["column"]), 0),
    ]
    .into_iter()
    .chain(
        OPERATORS
            .iter()
            .map(|operator| node(operator.0, operator.1, &[operator.2])),
    )
    .chain([along(node("Concat", &answers, &["probeOut"]), 1)])
    .fold(Message::default(), |graph, operation| graph.message(1, &operation))
    .text(2, "probes")
    .message(11, &row_of("probeIn", 8))
    .message(12, &row_of("probeOut", 16));
    // IR version 8 with operator set 17, the last in which `Split` divides its input evenly with no attribute.
    Message::default()
        .int(1, 8)
        .message(8, &Message::default().int(2, 17))
        .message(7, &graph)
        .0
}

/// What the graph behind `context` answers to the probe set.
#[expect(
    clippy::little_endian_bytes,
    reason = "the ONNX backend reads and writes a tensor's values little-endian"
)]
fn nn_signature(context: &mut ExecutionContext) -> Signature {
    let input = Tensor::new(
        vec![1, 8],
        TensorType::Fp32,
        INPUTS.iter().flat_map(|value| value.to_le_bytes()).collect(),
    );
    let outputs = context
        .compute_with_io(vec![NamedTensor {
            name: "probeIn".to_owned(),
            tensor: input,
        }])
        .unwrap();
    let output = outputs.first().unwrap();
    Signature::new(
        WASI_NN,
        output
            .tensor
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| Answer::Bits(u32::from_le_bytes(*word)))
            .collect(),
    )
}

#[test]
fn onnx_runtime_answers_every_probe_and_the_same_way_twice() {
    let model = probe_model();
    let mut backends = wasmtime_wasi_nn::backend::list();
    let graph = backends
        .first_mut()
        .unwrap()
        .load(&[&model], ExecutionTarget::Cpu)
        .unwrap();
    let mut context = graph.init_execution_context().unwrap();

    let signature = nn_signature(&mut context);
    let hardware = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("{WASI_NN} hardware: {hardware}");
    println!("{WASI_NN} fingerprint: {}", signature.fingerprint());
    println!("{WASI_NN} registry: {}", signature.identify().join(" | "));
    println!("{WASI_NN} traits: {}", signature.traits());
    println!("{WASI_NN} {}", signature.describe().join("\n"));

    assert_eq!(signature.completed(), PROBE_COUNT);
    assert!(!signature.traits().contains("=other"));
    assert_eq!(signature.fingerprint(), nn_signature(&mut context).fingerprint());
    // The platforms whose answers through ONNX Runtime are on record in the registry.
    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
    ))]
    assert!(signature.seen().unwrap().iter().any(et_gpu_probe::Sighting::is_here));

    // On macOS a process that has run an ONNX Runtime 1.22 session aborts as it exits, after the test has passed:
    //
    //   libc++abi: terminating due to uncaught exception of type
    //   std::__1::system_error: mutex lock failed: Invalid argument
    //   (test aborted with signal 6: SIGABRT)
    //
    // Seen on every run of this test on an Apple M5 Pro, at the commit below. The runner's `main` leaves by `_exit`
    // for the same abort, which skips the exit handlers it happens in, and so does this.
    //
    // https://github.com/edge-toolkit/core/commit/aba5d88c170c70fae30511f2ef6b44ad9a27fa1d
    #[cfg(target_os = "macos")]
    {
        use std::io::Write as _;

        std::io::stdout().flush().unwrap();
        #[expect(
            unsafe_code,
            reason = "libc::_exit is the only way to leave without running the exit handlers that abort"
        )]
        // SAFETY: `_exit` has no preconditions, and everything the test printed is flushed above.
        unsafe {
            libc::_exit(0);
        }
    }
}
