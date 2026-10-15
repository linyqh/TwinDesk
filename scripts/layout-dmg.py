#!/usr/bin/env python3
"""Write a portable Finder layout on the mounted DMG volume."""

import sys
from pathlib import Path

from ds_store import DSStore
from mac_alias import Alias, Bookmark

volume = Path(sys.argv[1]).resolve()
app_name = sys.argv[2]
background = str(volume / ".background" / "background.tiff")

with DSStore.open(str(volume / ".DS_Store"), "w+") as store:
    store["."]["bwsp"] = {
        "WindowBounds": "{{160, 160}, {768, 544}}",
        "ShowToolbar": False,
        "ShowStatusBar": False,
        "ShowSidebar": False,
        "ShowPathbar": False,
        "ShowTabView": False,
        "ContainerShowSidebar": False,
        "PreviewPaneVisibility": False,
    }
    store["."]["icvp"] = {
        "version": 1,
        "viewOptionsVersion": 1,
        "backgroundType": 2,
        "backgroundImageAlias": Alias.for_file(background).to_bytes(),
        "backgroundColorRed": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorBlue": 1.0,
        "iconSize": 88.0,
        "textSize": 13.0,
        "gridSpacing": 100.0,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "arrangeBy": "none",
        "labelOnBottom": True,
        "showIconPreview": True,
        "showItemInfo": False,
        "scrollPositionX": 0.0,
        "scrollPositionY": 0.0,
    }
    store["."]["pBBk"] = Bookmark.for_file(background)
    store["."]["vSrn"] = ("long", 1)
    store["."]["icvl"] = ("type", b"icnv")
    store[app_name]["Iloc"] = (193, 224)
    store["Applications"]["Iloc"] = (575, 224)
    # Keep support folders away from the artwork even when the user's Finder
    # is configured to show hidden files. Do not change that global preference.
    store[".background"]["Iloc"] = (2000, 2000)
    store[".fseventsd"]["Iloc"] = (2200, 2000)
