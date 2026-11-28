# Compute shader dual-path oracle (macOS)

Set `VICTORY_SHADER_EXEC` to a built Metal executor. Run `tools/mac/xdxc-oracle.sh` (optional test name prefix, e.g. `wave_prefix`). The helper accepts `VICTORY_HELPER_DXC` or `VICTORY_DXC_HELPER` and uses `DXC_LIB`. SDK absent: the integration cases print SKIPPED and return successfully. `VICTORY_GDKX_DIR` overrides the installation; `VICTORY_XDXC_RECOMPILE=1` requests recompilation. Keep compiler artifacts in the private cache outside the checkout.

`dual_path(hlsl, inputs)` compiles one compute body twice. A uses helper DXC; B uses the console NewBE compiler and the shipping translator. The harness dispatches both DXIL modules via MSC/Metal and compares the returned UAVs. Bind the same logical inputs in the same order using the prelude: A uses heap indices, B a descriptor table. A failed stage is not a shader mismatch. Check the printed verdict, not just the Cargo pass count: some cases deliberately characterize a difference.

Exact comparison is the default. Allocations may race; the allocation tests compare order-independent contents and counters. Half conversions canonicalize NaNs. The division diagnostic separately checks B against a reciprocal/product model with a two-ULP budget for the reciprocal and product roundings. The direct A/B comparison does not use that budget. Neither path executes on a physical console.

For wave tests, `VICTORY_XDXC_VERBOSE=1` prints translated DXIL keys. A pair of matching outputs **and matching program keys** cannot distinguish two translator forms. A runs with the host wave size; B-only tests of explicitly numbered high lanes are supplemental diagnostics, not A/B evidence.
