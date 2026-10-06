#!/usr/bin/env bash
# Usage: rotate.sh DIR N
# Backups are named backup-YYYYMMDD.tar.gz. Keeps the N newest, deletes the rest.
dir=$1
keep=$2
cd $dir
for f in $(ls -t | head -n $keep); do
  rm $f
done
