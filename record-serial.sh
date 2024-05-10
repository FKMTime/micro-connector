#!/bin/bash
stty -F /dev/ttyUSB0 115200
cat /dev/ttyUSB0 | ts | tee ~/logs.txt
