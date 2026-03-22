#!/bin/bash
# Aufruf: ./run-test.sh s3-benchmark
TESTNAME=$1
docker run -it --rm \
  -e SERVER_ENDPOINT=192.168.178.38:3000 \
  -e ACCESS_KEY=AKIAIOSFODNN7EXAMPLE \
  -e SECRET_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY \
  -e ENABLE_HTTPS=0 \
  -v /tmp/mint:/mint/log \
  mein-mint "$TESTNAME"

