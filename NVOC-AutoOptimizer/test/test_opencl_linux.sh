#! /bin/bash
echo $@
. ../NVOC-CLI-Stressor/.venv/bin/activate
python ../NVOC-CLI-Stressor/test.py --precisions fp32 --matrix-sizes 2048,4096,8192 --duration $(($2*5))
