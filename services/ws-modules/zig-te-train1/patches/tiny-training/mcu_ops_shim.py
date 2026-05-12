"""
Shim that registers the custom mcuconv2d / mcuadd / mcutruncate / mcumean ops
in pure Python on stock apache-tvm, replacing the C++ side of TVM-hack we
can't build on modern compilers.

Type-rel funcs use the stock-TVM Python convention: take (args, attrs) and
RETURN the output Type. (The mode-1 lookup via tvm.relay.type_relation.* with
reporter.assign() is C++-only.)

Import before any compilation/ module that touches relay.nn.mcuconv2d.
"""

import tvm
from tvm import relay
from tvm._ffi.registry import get_global_func, register_func

_register_op = get_global_func("ir.RegisterOp")


def _ensure_op(name, desc):
    try:
        _register_op(name, desc)
    except Exception:
        pass
    return tvm.ir.Op.get(name)


def _int(x, default=0):
    """Extract int from a tvm IntImm / Integer / plain int, with default."""
    if x is None:
        return default
    v = getattr(x, "value", x)
    try:
        return int(v)
    except Exception:
        return default


# ────────────────────────────────────────────────────────────────────────────
# nn.mcuconv2d
# ────────────────────────────────────────────────────────────────────────────
op_mcuconv2d = _ensure_op("nn.mcuconv2d", "MCU quantized conv2d with QAS metadata")
op_mcuconv2d.set_num_inputs(6)
op_mcuconv2d.set_attrs_type_key("relay.attrs.Conv2DAttrs")
op_mcuconv2d.set_support_level(2)


def _mcuconv2d_rel(args, attrs):
    """Python type-rel: returns the output TensorType for nn.mcuconv2d."""
    data = args[0]
    weight = args[1]
    if not hasattr(data, "shape") or not hasattr(weight, "shape"):
        return None

    layout = attrs.data_layout if hasattr(attrs, "data_layout") and attrs.data_layout else "NCHW"
    klayout = attrs.kernel_layout if hasattr(attrs, "kernel_layout") and attrs.kernel_layout else "OIHW"

    if layout == "NCHW":
        N, _, H, W = data.shape
    elif layout == "NHWC":
        N, H, W, _ = data.shape
    else:
        # Fallback: first dim is batch, last is channel
        return None

    if klayout in ("OIHW", "OHWI"):
        OC = weight.shape[0]
    elif klayout == "HWIO":
        OC = weight.shape[3]
    else:
        OC = weight.shape[0]

    if attrs.kernel_size:
        kh, kw = _int(attrs.kernel_size[0]), _int(attrs.kernel_size[1])
    else:
        if klayout in ("OIHW",):
            kh, kw = _int(weight.shape[2]), _int(weight.shape[3])
        elif klayout == "OHWI":
            kh, kw = _int(weight.shape[1]), _int(weight.shape[2])
        else:
            kh, kw = _int(weight.shape[1]), _int(weight.shape[2])

    sh = _int(attrs.strides[0], 1) if attrs.strides else 1
    sw = _int(attrs.strides[1], 1) if attrs.strides else 1

    pad = list(attrs.padding) if attrs.padding else [0, 0, 0, 0]
    if len(pad) == 1:
        pad = [pad[0]] * 4
    elif len(pad) == 2:
        pad = [pad[0], pad[1], pad[0], pad[1]]
    ph_top, pw_left, ph_bot, pw_right = (_int(p, 0) for p in pad[:4])

    OH = (_int(H) + ph_top + ph_bot - kh) // sh + 1
    OW = (_int(W) + pw_left + pw_right - kw) // sw + 1

    # TVM-hack mcuconv2d emits int32 (downstream mcutruncate clips back to
    # int8). The tinyengine parser uses output_dtype == "int32" to discriminate
    # depthwise-int8 mcuconv2d from the generic group_conv2d codegen path
    # (see TTEParser.py: depthwiseConv2d vs groupConv2d dispatch). Default to
    # int32 unless attrs.out_dtype is set explicitly.
    out_dtype = attrs.out_dtype if attrs.out_dtype else "int32"
    if layout == "NCHW":
        out_shape = (N, OC, OH, OW)
    else:
        out_shape = (N, OH, OW, OC)

    return relay.TensorType(out_shape, out_dtype)


op_mcuconv2d.add_type_rel("MCUConv2D", _mcuconv2d_rel)


def _py_make_mcuconv2d(
    data,
    weight,
    bias,
    zero_x,
    zero_y,
    effective_scale,
    strides,
    padding,
    dilation,
    groups,
    channels,
    kernel_size,
    data_layout,
    kernel_layout,
    out_layout,
    out_dtype,
):
    attrs = tvm.ir.make_node(
        "relay.attrs.Conv2DAttrs",
        strides=strides,
        padding=padding,
        dilation=dilation,
        groups=groups,
        channels=channels,
        kernel_size=kernel_size,
        data_layout=data_layout,
        kernel_layout=kernel_layout,
        out_layout=out_layout,
        out_dtype=out_dtype,
    )
    return relay.Call(tvm.ir.Op.get("nn.mcuconv2d"), [data, weight, bias, zero_x, zero_y, effective_scale], attrs)


register_func("relay.op.nn._make.mcuconv2d", _py_make_mcuconv2d, override=True)


# ────────────────────────────────────────────────────────────────────────────
# nn.mcuadd
# ────────────────────────────────────────────────────────────────────────────
op_mcuadd = _ensure_op("nn.mcuadd", "MCU quantized add with QAS metadata")
op_mcuadd.set_num_inputs(8)
op_mcuadd.set_attrs_type_key("relay.attrs.BiasAddAttrs")
op_mcuadd.set_support_level(2)


def _mcuadd_rel(args, attrs):
    x1 = args[0]
    return relay.TensorType(x1.shape, x1.dtype)


op_mcuadd.add_type_rel("MCUAdd", _mcuadd_rel)


def _py_make_mcuadd(x1, x2, zero_x1, zero_x2, scale_x1, scale_x2, zero_y, scale_y, out_dtype=""):
    attrs = tvm.ir.make_node("relay.attrs.BiasAddAttrs", axis=1)
    return relay.Call(
        tvm.ir.Op.get("nn.mcuadd"), [x1, x2, zero_x1, zero_x2, scale_x1, scale_x2, zero_y, scale_y], attrs
    )


register_func("relay.op.nn._make.mcuadd", _py_make_mcuadd, override=True)


# ────────────────────────────────────────────────────────────────────────────
# nn.mcutruncate
# ────────────────────────────────────────────────────────────────────────────
op_mcutrunc = _ensure_op("nn.mcutruncate", "MCU clip/saturate to int8 range")
op_mcutrunc.set_num_inputs(1)
op_mcutrunc.set_attrs_type_key("relay.attrs.ClipAttrs")
op_mcutrunc.set_support_level(2)


def _mcutrunc_rel(args, attrs):
    inp = args[0]
    out_dtype = "int8"
    if hasattr(attrs, "out_dtype") and attrs.out_dtype:
        out_dtype = attrs.out_dtype
    return relay.TensorType(inp.shape, out_dtype)


op_mcutrunc.add_type_rel("MCUTruncate", _mcutrunc_rel)


def _py_make_mcutruncate(data, out_dtype="int8"):
    # TVM-hack defined a custom TruncateAttrs with min/max/out_dtype fields.
    # Stock TVM doesn't have that attrs class. Use ClipAttrs (a_min/a_max are
    # the analogous fields) and attach -128/+127 since this op is always
    # int8 truncation in the MCUNetV3 pipeline. The autodiff gradient code
    # reads .min and .max — see _patch_mcutruncate_grad below for the
    # bridge.
    attrs = tvm.ir.make_node("relay.attrs.ClipAttrs", a_min=-128.0, a_max=127.0)
    return relay.Call(tvm.ir.Op.get("nn.mcutruncate"), [data], attrs)


register_func("relay.op.nn._make.mcutruncate", _py_make_mcutruncate, override=True)


# Monkey-patch the autodiff gradient registration for nn.mcutruncate so it
# reads from ClipAttrs.a_min / a_max instead of the custom TruncateAttrs
# fields the TVM-hack version expected. Without this the bwd graph fails
# with `AttributeError: 'NoneType' object has no attribute 'min'`.
def _patch_mcutruncate_grad():
    from tvm.relay.op import op as _op_mod

    # Re-register the gradient with our attrs convention
    @_op_mod.register_gradient("nn.mcutruncate", level=20)
    def _mcutruncate_grad_patched(orig, grad):
        new_inputs = [relay.cast(a, "float32") for a in orig.args]
        x = new_inputs[0]
        dtype = "float32"
        a_min = getattr(orig.attrs, "a_min", -128.0) if orig.attrs is not None else -128.0
        a_max = getattr(orig.attrs, "a_max", 127.0) if orig.attrs is not None else 127.0
        lo = relay.const(float(a_min), dtype=dtype)
        hi = relay.const(float(a_max), dtype=dtype)
        mask1 = relay.greater_equal(x, lo)
        mask2 = relay.less_equal(x, hi)
        mask = mask1 * mask2
        zeros = relay.zeros_like(grad)
        return [relay.where(mask, grad, zeros)]


_patch_mcutruncate_grad()


# ────────────────────────────────────────────────────────────────────────────
# mcumean  (note: no nn. prefix, per the original)
# ────────────────────────────────────────────────────────────────────────────
op_mcumean = _ensure_op("mcumean", "MCU global average pool (quantized mean)")
op_mcumean.set_num_inputs(1)
op_mcumean.set_attrs_type_key("relay.attrs.ReduceAttrs")
op_mcumean.set_support_level(2)


def _mcumean_rel(args, attrs):
    inp = args[0]
    axes = list(getattr(attrs, "axis", []) or [])
    keepdims = bool(getattr(attrs, "keepdims", False))
    axes = [_int(a) for a in axes]
    in_shape = list(inp.shape)
    ndim = len(in_shape)
    axes = [a if a >= 0 else a + ndim for a in axes]
    out_shape = []
    for i, dim in enumerate(in_shape):
        if i in axes:
            if keepdims:
                out_shape.append(1)
        else:
            out_shape.append(dim)
    return relay.TensorType(tuple(out_shape), inp.dtype)


op_mcumean.add_type_rel("MCUMean", _mcumean_rel)


def _py_make_mcumean(data, axis=None, keepdims=False, exclude=False):
    if axis is None:
        axis = []
    if isinstance(axis, int):
        axis = [axis]
    attrs = tvm.ir.make_node("relay.attrs.ReduceAttrs", axis=axis, keepdims=keepdims, exclude=exclude)
    return relay.Call(tvm.ir.Op.get("mcumean"), [data], attrs)


register_func("relay.op._make.mcumean", _py_make_mcumean, override=True)


# ────────────────────────────────────────────────────────────────────────────
# Python wrappers on tvm.relay.* namespaces so call sites resolve.
# ────────────────────────────────────────────────────────────────────────────
from tvm.relay.op.nn.utils import get_pad_tuple2d  # type: ignore
import tvm.relay as _relay_mod
import tvm.relay.op as _relay_op
import tvm.relay.op.nn as _relay_nn


def mcuconv2d(
    data,
    weight,
    bias,
    zero_x,
    zero_y,
    effective_scale,
    strides=(1, 1),
    padding=(0, 0),
    dilation=(1, 1),
    groups=1,
    channels=None,
    kernel_size=None,
    data_layout="NCHW",
    kernel_layout="OIHW",
    out_layout="",
    out_dtype="",
):
    if isinstance(kernel_size, int):
        kernel_size = (kernel_size, kernel_size)
    if isinstance(strides, int):
        strides = (strides, strides)
    if isinstance(dilation, int):
        dilation = (dilation, dilation)
    padding = get_pad_tuple2d(padding)
    return _py_make_mcuconv2d(
        data,
        weight,
        bias,
        zero_x,
        zero_y,
        effective_scale,
        strides,
        padding,
        dilation,
        groups,
        channels,
        kernel_size,
        data_layout,
        kernel_layout,
        out_layout,
        out_dtype,
    )


def mcuadd(x1, x2, zero_x1, zero_x2, scale_x1, scale_x2, zero_y, scale_y, out_dtype=""):
    return _py_make_mcuadd(x1, x2, zero_x1, zero_x2, scale_x1, scale_x2, zero_y, scale_y, out_dtype)


def mcutruncate(data, out_dtype="int8"):
    return _py_make_mcutruncate(data, out_dtype)


def mcumean(data, axis=None, keepdims=False, exclude=False):
    return _py_make_mcumean(data, axis=axis, keepdims=keepdims, exclude=exclude)


_relay_mod.mcumean = mcumean
_relay_op.mcumean = mcumean
_relay_nn.mcumean = mcumean
_relay_nn.mcuconv2d = mcuconv2d
_relay_nn.mcuadd = mcuadd
_relay_nn.mcutruncate = mcutruncate


def _check():
    for name in ("nn.mcuconv2d", "nn.mcuadd", "nn.mcutruncate", "mcumean"):
        op = tvm.ir.Op.get(name)
        print(f"  {name}: num_inputs={op.num_inputs}, support_level={op.support_level}")


if __name__ == "__main__":
    print("MCU ops registered:")
    _check()
