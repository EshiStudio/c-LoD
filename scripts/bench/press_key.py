#!/usr/bin/env python3
"""Send keys to the X display via XTest (for the headless bench client).
Usage: press_key.py <keycode> [keycode ...]   (Return=36, Tab=23)"""
import sys
import time

from Xlib import X, display
from Xlib.ext import xtest

d = display.Display(":99")
for arg in sys.argv[1:]:
    kc = int(arg)
    xtest.fake_input(d, X.KeyPress, kc)
    d.sync()
    time.sleep(0.08)
    xtest.fake_input(d, X.KeyRelease, kc)
    d.sync()
    time.sleep(0.12)
    print(f"key {kc} sent")
