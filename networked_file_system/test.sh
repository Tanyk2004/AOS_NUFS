#!/usr/bin/env bash
set -euo pipefail

# I made GPT write this script to clear the cache directory before running the tester.
# My signal handler was infinitely blocking Idk why :crying cat:

CACHE_DIR="/var/tmp/tulfs_cache"

# Delete every child of CACHE_DIR without removing the directory itself
if [[ -d "$CACHE_DIR" ]]; then
    find "$CACHE_DIR" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
fi

sudo ./target/debug/client /mnt/netfs/ tanay24@c220g5-110410.wisc.cloudlab.us:/tmp/fs_dir
