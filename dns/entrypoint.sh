#!/bin/bash

SCRIPT_DIR=$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )
ENVSUBST_VARS=$(env | grep -o "^[^=]\+" | sed -e 's/^/\$/g' | tr '\n' ' ')
cd $SCRIPT_DIR

envsubst "$ENVSUBST_VARS" < named.conf > /etc/bind/named.conf
for f in *.zone; do 
    envsubst "$ENVSUBST_VARS" < $f > /etc/bind/$f
done

/usr/local/bin/docker-entrypoint.sh
