#!/usr/bin/env bash
# Simple script to check switch config via serial
stty -F /dev/serial_aruba-2930F 9600 cs8 -cstopb -parenb
(
  sleep 1
  echo ""
  sleep 1
  echo "show run | include interface [1-4]"
  sleep 2
  echo "show run | begin interface 1"
  sleep 3
  echo "exit"
  sleep 1
) > /dev/serial_aruba-2930F &
cat /dev/serial_aruba-2930F | timeout 10 cat
