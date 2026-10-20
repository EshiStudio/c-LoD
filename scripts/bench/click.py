#!/usr/bin/env python3
"""Click at absolute screen coords on :99 via XTest. Usage: click.py <x> <y>"""
import sys
import time

from Xlib import X, display
from Xlib.ext import xtest

x, y = int(sys.argv[1]), int(sys.argv[2])
d = display.Display(":99")
xtest.fake_input(d, X.MotionNotify, x=x, y=y)
d.sync()
time.sleep(0.15)
xtest.fake_input(d, X.ButtonPress, 1)
d.sync()
time.sleep(0.1)
xtest.fake_input(d, X.ButtonRelease, 1)
d.sync()
print(f"clicked at {x},{y}")
