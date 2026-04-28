#! /bin/bash
echo $@
. ../NVOC-CLI-Stressor/.venv/bin/activate
python ../NVOC-CLI-Stressor/test.py --precisions fp32 --matrix-sizes 16384 --duration 60 --jitter-rate 0
