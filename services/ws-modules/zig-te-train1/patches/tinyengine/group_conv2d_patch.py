"""
Method-replacement wrapper that fixes tinyengine's `groupConv2d.generate_inference_str`.

Upstream tinyengine raises NotImplementedError when (groups == in_c == out_c)
and `out_c / groups == 1` — i.e. a depthwise-equivalent gradient conv that
doesn't have a 4/8/16-aligned ratio and isn't a 10-class pointwise. tinyengine
already wires the depthwise_conv_fp_kernel*_uniweight_1row1col path through
the upstream function-name dispatcher; the inplace check at the end just
forgot to allow this branch.

We monkey-patch by overwriting `groupConv2d.generate_inference_str` with our
own version. The body is a copy of the upstream method with a single `elif`
inserted in the inplace check. No file in vendor/upstream/tinyengine/ is
modified — only the in-memory class binding is replaced at import time.

This module must be imported BEFORE tinyengine's CodeGenerator processes any
op, but AFTER `code_generator.operators.group_conv2d` exists in sys.modules.
The stage2_wrapper.py orchestrator handles that ordering.
"""

from code_generator.operators.group_conv2d import groupConv2d
from code_generator.operators.basic_utils import isweightstr


def _patched_generate_inference_str(self, tflite_op=False, dummy_address=False):
    string = ""
    params = self.params
    # floating point implementation
    if tflite_op:
        string += f"conv_params.stride_height = {params['stride_h']};\n"
        string += f"conv_params.stride_width = {params['stride_w']};\n"
        string += f"conv_params.dilation_width_factor = {params['dilation_w']};\n"
        string += f"conv_params.dilation_height_factor = {params['dilation_h']};\n"
        string += f"conv_params.input_offset = {params['input_zero_point']};\n"
        string += f"conv_params.output_offset = {params['output_zero_point']};\n"
        string += f"conv_params.padding_values.width = {params['padding_w']};\n"
        string += f"conv_params.padding_values.height = {params['padding_h']};\n"
        string += "conv_params.quantized_activation_min = -128;\n"
        string += "conv_params.quantized_activation_max = 127;\n"
        string += f"conv_params.float_activation_min = {params['float_min']};\n"
        string += f"conv_params.float_activation_max = {params['float_max']};\n"

        if isinstance(params["weight_name"], str) and isweightstr(params["weight_name"]):
            weight_string = params["weight_name"]
        else:
            weight_string = f"weight_fp{params['parsed_trainable']}"

        function_name = "group_conv"
        if params["input_dtype"] == "int8":
            function_name += "_int8input"

        if not params["float32_input2"]:
            function_name += "_int8weight"

        if params["inplace_weight_name"] is not None:
            if self.params["QAS"] is not None:
                QAS_cnt = int(self.params["output_c"] / self.params["input_c"])
                if QAS_cnt == 1:
                    QAS_cnt = len(self.params["QAS"].flatten())
                string += f"const float {self.params['inplace_weight_name']}_QAS[{QAS_cnt}] = " + "{"
                QAS = self.params["QAS"].flatten()
                for i in 1 / QAS:
                    string += str(i) + ","
                string += "};\n"

                if dummy_address:
                    string += (
                        f"{function_name}_inplace(conv_params,{params['groups']},&buffer0[0],"
                        + f"{params['input_h']},{params['input_w']},{params['input_c']},"
                        + f"{weight_string},{params['kernel_h']},{params['kernel_w']},NULL,"
                        + f"{params['inplace_weight_name']},"
                        + f"{str(params['output_h'])},{str(params['output_w'])},"
                        + f"{str(params['output_c'])},(float*)sbuf,1, "
                        + f"{self.params['inplace_weight_name']}_QAS, lr);\n"
                    )
                else:
                    string += (
                        f"{function_name}_inplace(conv_params,{params['groups']},"
                        + f"{self._getBufferstrCast(params['input_buf_add'], params['input_buf_add_offset'])},"
                        + f"{params['input_h']},{params['input_w']},{params['input_c']},"
                        + f"{weight_string},{params['kernel_h']},{params['kernel_w']},NULL,"
                        + f"{params['inplace_weight_name']},"
                        + f"{str(params['output_h'])},{str(params['output_w'])},{str(params['output_c'])},"
                        + f"(float*)sbuf,1, {self.params['inplace_weight_name']}_QAS, lr);\n"
                    )
        else:
            string += (
                f"{function_name}(conv_params,{params['groups']},"
                + f"{self._getBufferstrCast(params['input_buf_add'], params['input_buf_add_offset'])},"
                + f"{params['input_h']},{params['input_w']},{params['input_c']},"
                + f"{weight_string},{params['kernel_h']},{params['kernel_w']},NULL,"
                + f"{self._getBufferstrCast(params['output_buf_add'], params['output_buf_add_offset'])},"
                + f"{str(params['output_h'])},{str(params['output_w'])},"
                + f"{str(params['output_c'])},(float*)sbuf,1);\n"
            )
    elif not tflite_op:
        # function name
        if (
            params["kernel_h"] == 1
            and params["kernel_w"] == 1
            and params["input_h"] == 1
            and params["input_w"] == 1
            and params["output_h"] == 1
            and params["output_w"] == 1
            and params["output_c"] / params["input_c"] == 10
        ):  # group pointwise conv
            function_name = (
                "group_pointwise_conv_in1x1_out1x1_1row10col_uniweight"
                if not params["float32_input2"]
                else "group_pointwise_conv_fp_in1x1_out1x1_1row10col_uniweight"
            )
        elif (
            params["output_c"] == params["input_c"] and params["output_c"] == params["groups"]
        ):  # Same like depthwise conv
            function_name = "depthwise_conv_kernel" if not params["float32_input2"] else "depthwise_conv_fp_kernel"
            function_name += (
                f"{str(params['kernel_h'])}_stride{str(params['stride_h'])}_pad{str(params['padding_h'])}"
                + f"_in{str(params['input_h'])}x{str(params['input_w'])}_out{str(params['output_h'])}x"
                + f"{str(params['output_w'])}_uniweight_1row1col"
            )
        elif (params["output_c"] / params["groups"]) % 16 == 0:
            function_name = "group_conv_kernel" if not params["float32_input2"] else "group_conv_fp_kernel"
            function_name += (
                f"{str(params['kernel_h'])}_stride{str(params['stride_h'])}_pad{str(params['padding_h'])}_in"
                + f"{str(params['input_h'])}x{str(params['input_w'])}_out{str(params['output_h'])}x"
                + f"{str(params['output_w'])}_uniweight_4row16col"
            )
        elif (params["output_c"] / params["groups"]) % 8 == 0:
            function_name = "group_conv_kernel" if not params["float32_input2"] else "group_conv_fp_kernel"
            function_name += (
                f"{str(params['kernel_h'])}_stride{str(params['stride_h'])}_pad{str(params['padding_h'])}_in"
                + f"{str(params['input_h'])}x{str(params['input_w'])}_out{str(params['output_h'])}x"
                + f"{str(params['output_w'])}_uniweight_4row8col"
            )
        else:
            raise NotImplementedError

        # int8 input for inplace cast
        if params["input_dtype"] == "int8":
            function_name += "_int8input"

        if not params["float32_input2"]:
            function_name += "_int8weight"

        # ── PATCH (vs upstream): allow `_inplace` for the depthwise-equivalent
        #    case (groups == in == out, ratio = 1). The upstream check only
        #    accepts %16, %8, or the 10-class pointwise — but the depthwise
        #    function template already exists for this case via the second
        #    branch above (line ~117), so the suffix is appropriate. Without
        #    this, sparse_bp on mbv2/proxyless hits NotImplementedError.
        if (
            (params["output_c"] / params["groups"]) % 16 == 0
            or (params["output_c"] / params["groups"]) % 8 == 0
            or params["output_c"] / params["input_c"] == 10
        ):
            function_name += "_inplace"
        elif params["output_c"] == params["input_c"] == params["groups"]:
            function_name += "_inplace"
        else:
            raise NotImplementedError
        # ── END PATCH

        # weight name
        if isinstance(params["weight_name"], str) and isweightstr(params["weight_name"]):
            weight_string = params["weight_name"]
        else:
            weight_string = f"weight_fp{params['parsed_trainable']}"

        # require int32 output buffer
        norm_buffer_add = None
        if params["norm_buffer"]:
            norm_tensor = self.input_tensors[params["norm_buffer"]]
            norm_buffer_add = f"&{norm_tensor.buffer_name}[{norm_tensor.buffer_address}]"

        if params["inplace_weight_name"] is not None:
            if self.params["QAS"] is not None:
                QAS_cnt = int(self.params["output_c"] / self.params["input_c"])
                if QAS_cnt == 1:
                    QAS_cnt = len(self.params["QAS"].flatten())
                string += f"const float {self.params['inplace_weight_name']}_QAS[{QAS_cnt}] = " + "{"
                QAS = self.params["QAS"].flatten()
                for i in 1 / QAS:
                    string += str(i) + ","
                string += "};\n"

                string += (
                    f"{function_name}"
                    + f"({self._getBufferstrCast(params['input_buf_add'], params['input_buf_add_offset'])},"
                    + f"{params['input_h']},{params['input_w']},{params['input_c']},"
                    + f"{weight_string},NULL,"
                    + f"{params['inplace_weight_name']},"
                    + f"{str(params['output_h'])},{str(params['output_w'])},{str(params['output_c'])},"
                    + f"{params['float_min']},{params['float_max']},"
                )
                if not params["float32_input2"]:
                    string += (
                        (
                            f"(float*)sbuf, NULL, 1,{params['groups']}, "
                            + f"{self.params['inplace_weight_name']}_QAS, lr);\n"
                        )
                        if not norm_buffer_add
                        else (
                            f"(float*)sbuf, {norm_buffer_add}, 1, {params['groups']}, "
                            + f"{self.params['inplace_weight_name']}_QAS, lr);\n"
                        )
                    )
                else:
                    string += f"(float*)sbuf,1,{params['groups']}, {self.params['inplace_weight_name']}_QAS, lr);\n"
            else:
                string += (
                    f"{function_name}"
                    + f"({self._getBufferstrCast(params['input_buf_add'], params['input_buf_add_offset'])},"
                    + f"{params['input_h']},{params['input_w']},{params['input_c']},"
                    + f"{weight_string},NULL,"
                    + f"{params['inplace_weight_name']},"
                    + f"{str(params['output_h'])},{str(params['output_w'])},{str(params['output_c'])},"
                    + f"{params['float_min']},{params['float_max']},"
                )
                if not params["float32_input2"]:
                    string += (
                        f"(float*)sbuf, NULL, 1,{params['groups']});\n"
                        if not norm_buffer_add
                        else f"(float*)sbuf, {norm_buffer_add}, 1, {params['groups']});\n"
                    )
                else:
                    string += f"(float*)sbuf,1,{params['groups']});\n"
        else:
            string += (
                f"{function_name}"
                + f"({self._getBufferstrCast(params['input_buf_add'], params['input_buf_add_offset'])},"
                + f"{params['input_h']},{params['input_w']},{params['input_c']},"
                + f"{weight_string},NULL,"
                + f"{self._getBufferstrCast(params['output_buf_add'], params['output_buf_add_offset'])},"
                + f"{str(params['output_h'])},{str(params['output_w'])},{str(params['output_c'])},"
                + f"{params['float_min']},{params['float_max']},"
            )
            if not params["float32_input2"]:
                string += (
                    f"(float*)sbuf,NULL,1,{params['groups']});\n"
                    if not norm_buffer_add
                    else f"(float*)sbuf, {norm_buffer_add}, 1, {params['groups']});\n"
                )
            else:
                string += f"(float*)sbuf,1,{params['groups']});\n"

    return string


# Apply the monkey-patch. After this import, groupConv2d.generate_inference_str
# refers to our patched version; the original is no longer reachable from
# Python-land but the underlying file on disk is unchanged.
groupConv2d.generate_inference_str = _patched_generate_inference_str
