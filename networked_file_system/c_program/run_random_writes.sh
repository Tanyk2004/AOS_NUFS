#!/usr/bin/env bash
set -euo pipefail

for n in $(seq 1500000 500000 3500000); do
    echo "Running random_writes with size $n"
    sudo ./random_writes /users/tanay24/mnt/nfs/bigfile "$n"
    sudo ./random_writes /mnt/netfs/bigfile "$n"
done

