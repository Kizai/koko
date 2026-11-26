"""Regenerate FinderLayout.dsstore after a layout change with ds_store==1.3.4."""

import os
from pathlib import Path

from ds_store import DSStore

# Finder stores this window in the main display's point space with its origin at the
# bottom edge, so the centered stored value equals the centered top offset.
screen_width = int(os.environ.get("KOKO_SCREEN_WIDTH", 1512))
screen_height = int(os.environ.get("KOKO_SCREEN_HEIGHT", 982))
window_width, window_height = 480, 300
window_x = max(0, (screen_width - window_width) // 2)
window_y = max(0, (screen_height - window_height) // 2)


with DSStore.open(str(Path(__file__).with_name("FinderLayout.dsstore")), "w+") as layout:
    layout["."]["vSrn"] = ("long", 1)
    layout["."]["icvl"] = ("type", b"icnv")
    layout["."]["bwsp"] = {
        "WindowBounds": f"{{{{{window_x}, {window_y}}}, {{{window_width}, {window_height}}}}}",
        "ShowToolbar": False,
        "ShowSidebar": False,
        "ContainerShowSidebar": False,
        "ShowStatusBar": False,
        "ShowPathbar": False,
        "ShowTabView": False,
    }
    layout["."]["icvp"] = {
        "viewOptionsVersion": 1,
        "backgroundType": 0,
        "backgroundColorRed": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorBlue": 1.0,
        "arrangeBy": "none",
        "gridSpacing": 64.0,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "labelOnBottom": True,
        "showItemInfo": False,
        "showIconPreview": False,
        "textSize": 16.0,
        "iconSize": 128.0,
    }
    layout["双击安装 KoKo.pkg"]["Iloc"] = (240, 125)
