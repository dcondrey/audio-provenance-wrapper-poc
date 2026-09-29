{
  "patcher": {
    "fileversion": 1,
    "appversion": { "major": 7, "minor": 0, "revision": 0, "architecture": "x64", "modernui": 1 },
    "rect": [100.0, 100.0, 640.0, 480.0],
    "boxes": [
      { "box": { "id": "obj-1", "maxclass": "newobj", "text": "cycle~ 440", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [30.0, 40.0, 70.0, 22.0] } },
      { "box": { "id": "obj-2", "maxclass": "newobj", "text": "sfplay~ drums/kick.wav", "numinlets": 2, "numoutlets": 2, "patching_rect": [30.0, 80.0, 160.0, 22.0] } },
      { "box": { "id": "obj-3", "maxclass": "ezdac~", "numinlets": 2, "numoutlets": 0, "patching_rect": [30.0, 200.0, 45.0, 45.0] } },
      { "box": { "id": "obj-4", "maxclass": "comment", "text": "a note", "numinlets": 1, "numoutlets": 0, "patching_rect": [200.0, 40.0, 60.0, 20.0] } },
      {
        "box": {
          "id": "obj-5",
          "maxclass": "newobj",
          "text": "p voice",
          "numinlets": 1,
          "numoutlets": 1,
          "patching_rect": [30.0, 120.0, 50.0, 22.0],
          "patcher": {
            "fileversion": 1,
            "appversion": { "major": 7, "minor": 0, "revision": 0, "architecture": "x64", "modernui": 1 },
            "rect": [0.0, 0.0, 300.0, 200.0],
            "boxes": [
              { "box": { "id": "obj-1", "maxclass": "newobj", "text": "*~ 0.5", "numinlets": 2, "numoutlets": 1, "patching_rect": [10.0, 40.0, 40.0, 22.0] } }
            ],
            "lines": []
          }
        }
      }
    ],
    "lines": [
      { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } },
      { "patchline": { "source": ["obj-2", 0], "destination": ["obj-3", 1] } }
    ]
  }
}
