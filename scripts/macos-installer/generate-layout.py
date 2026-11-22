"""Regenerate FinderLayout.dsstore after a layout change with ds_store==1.3.4."""

from pathlib import Path

from ds_store import DSStore


with DSStore.open(str(Path(__file__).with_name("FinderLayout.dsstore")), "w+") as layout:
    layout["."]["vSrn"] = ("long", 1)
    layout["."]["icvl"] = ("type", b"icnv")
    layout["."]["bwsp"] = {
        "WindowBounds": "{{120, 120}, {480, 300}}",
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
