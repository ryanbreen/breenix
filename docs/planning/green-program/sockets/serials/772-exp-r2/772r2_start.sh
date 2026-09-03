#!/bin/bash
cd /root || exit 1
nohup bash /root/772r2_driver.sh > /root/772r2_driver.out 2>&1 < /dev/null &
disown
echo "STARTED pid=$!"
