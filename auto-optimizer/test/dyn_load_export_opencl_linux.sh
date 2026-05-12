#! /bin/bash
echo $@
. ../.venv/bin/activate
python ../cli-stressor-opencl/test.py --precisions fp32 --matrix-sizes 16384 --duration 60 --jitter-rate 0
