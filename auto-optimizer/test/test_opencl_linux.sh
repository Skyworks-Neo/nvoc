#! /bin/bash
echo $@
. ../cli-stressor-opencl/.venv/bin/activate
python ../cli-stressor-opencl/test.py --precisions fp32 --matrix-sizes 2048,4096,8192 --duration $(($2*5))
