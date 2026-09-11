autowatch = 1;
outlets = 1;

var MAX_TRACKS = 64;
var MAX_CLIP_SLOTS_PER_TRACK = 32;
var MAX_DEVICES_PER_TRACK = 32;
var MAX_PARAMETERS_PER_DEVICE = 24;
var observers = [];

function bang() {
    probe();
}

function probe() {
    var result = build_session_probe();
    emit_json(result);
}

function start_observing() {
    stop_observing();
    observers.push(make_observer("live_set", "tracks"));
    observers.push(make_observer("live_set view", "selected_track"));
    post("[ableton_bridge] observing live_set tracks and selected_track. Send stop_observing to stop.\n");
}

function stop_observing() {
    observers = [];
}

function build_session_probe() {
    var limitations = [
        "This probe reads only fields exposed by Ableton Live API / Live Object Model.",
        "This probe does not prove that a clip or device contributed audio to a final export.",
        "Track, clip, device, and parameter values are session metadata, not audio-buffer evidence.",
        "Third-party plugin internals and preset state are not exposed unless Live exposes them as device parameters.",
        "Exact provenance relationships between imported samples, routed plugin audio, and final exports remain unverified here."
    ];

    var liveSet = make_api("live_set");
    var trackCount = safe_getcount(liveSet, "tracks");
    var selectedTrack = read_selected_track();
    var tracks = [];

    if (trackCount === null) {
        limitations.push("Could not read live_set tracks count.");
        trackCount = 0;
    }

    if (trackCount > MAX_TRACKS) {
        limitations.push("Track scan was capped at " + MAX_TRACKS + " tracks.");
    }

    for (var trackIndex = 0; trackIndex < Math.min(trackCount, MAX_TRACKS); trackIndex += 1) {
        tracks.push(read_track(trackIndex, limitations));
    }

    return {
        event_type: "ableton_session_probe",
        proof_level: "directly_observed_via_live_api",
        observed_at_local: new Date().toISOString(),
        live_api_root: "live_set",
        track_count_observed: trackCount,
        selected_track: selectedTrack,
        tracks: tracks,
        limitations: limitations
    };
}

function read_selected_track() {
    var selectedApi = make_api("live_set view selected_track");
    if (!is_valid_api(selectedApi)) {
        return {
            track_name: null,
            live_api_path: "live_set view selected_track",
            accessible: false
        };
    }

    return {
        track_name: safe_getstring(selectedApi, "name"),
        live_api_path: selectedApi.unquotedpath || "live_set view selected_track",
        live_api_id: selectedApi.id,
        accessible: true
    };
}

function read_track(trackIndex, limitations) {
    var trackPath = "live_set tracks " + trackIndex;
    var trackApi = make_api(trackPath);

    var deviceCount = safe_getcount(trackApi, "devices");
    var clipSlotCount = safe_getcount(trackApi, "clip_slots");
    var devices = [];
    var clips = [];

    if (deviceCount === null) {
        limitations.push("Could not read device count for track " + trackIndex + ".");
        deviceCount = 0;
    }

    if (clipSlotCount === null) {
        limitations.push("Could not read clip slot count for track " + trackIndex + ".");
        clipSlotCount = 0;
    }

    if (deviceCount > MAX_DEVICES_PER_TRACK) {
        limitations.push("Device scan for track " + trackIndex + " was capped at " + MAX_DEVICES_PER_TRACK + " devices.");
    }

    if (clipSlotCount > MAX_CLIP_SLOTS_PER_TRACK) {
        limitations.push("Clip slot scan for track " + trackIndex + " was capped at " + MAX_CLIP_SLOTS_PER_TRACK + " slots.");
    }

    for (var deviceIndex = 0; deviceIndex < Math.min(deviceCount, MAX_DEVICES_PER_TRACK); deviceIndex += 1) {
        devices.push(read_device(trackPath, deviceIndex, limitations));
    }

    for (var clipSlotIndex = 0; clipSlotIndex < Math.min(clipSlotCount, MAX_CLIP_SLOTS_PER_TRACK); clipSlotIndex += 1) {
        var clip = read_clip_slot(trackPath, clipSlotIndex);
        if (clip !== null) {
            clips.push(clip);
        }
    }

    return {
        track_index: trackIndex,
        track_name: safe_getstring(trackApi, "name"),
        live_api_path: trackPath,
        clip_slot_count_observed: clipSlotCount,
        device_count_observed: deviceCount,
        devices: devices,
        clips: clips
    };
}

function read_device(trackPath, deviceIndex, limitations) {
    var devicePath = trackPath + " devices " + deviceIndex;
    var deviceApi = make_api(devicePath);
    var parameterCount = safe_getcount(deviceApi, "parameters");
    var parameters = [];
    var className = safe_getstring(deviceApi, "class_name");

    if (parameterCount === null) {
        limitations.push("Could not read parameter count for " + devicePath + ".");
        parameterCount = 0;
    }

    if (parameterCount > MAX_PARAMETERS_PER_DEVICE) {
        limitations.push("Parameter scan for " + devicePath + " was capped at " + MAX_PARAMETERS_PER_DEVICE + " parameters.");
    }

    for (var parameterIndex = 0; parameterIndex < Math.min(parameterCount, MAX_PARAMETERS_PER_DEVICE); parameterIndex += 1) {
        parameters.push(read_parameter(devicePath, parameterIndex));
    }

    return {
        device_index: deviceIndex,
        name: safe_getstring(deviceApi, "name"),
        class_name: className,
        class_display_name: safe_getstring(deviceApi, "class_display_name"),
        type: safe_get(deviceApi, "type"),
        likely_third_party_plugin: className === "PluginDevice",
        parameters: parameters
    };
}

function read_parameter(devicePath, parameterIndex) {
    var parameterPath = devicePath + " parameters " + parameterIndex;
    var parameterApi = make_api(parameterPath);

    return {
        parameter_index: parameterIndex,
        name: safe_getstring(parameterApi, "name"),
        value: safe_get(parameterApi, "value"),
        display_value: safe_getstring(parameterApi, "display_value"),
        min: safe_get(parameterApi, "min"),
        max: safe_get(parameterApi, "max"),
        is_enabled: safe_bool(safe_get(parameterApi, "is_enabled"))
    };
}

function read_clip_slot(trackPath, clipSlotIndex) {
    var slotPath = trackPath + " clip_slots " + clipSlotIndex;
    var slotApi = make_api(slotPath);
    var hasClip = safe_bool(safe_get(slotApi, "has_clip"));

    if (!hasClip) {
        return null;
    }

    var clipPath = slotPath + " clip";
    var clipApi = make_api(clipPath);
    var isAudioClip = safe_bool_or_null(safe_get(clipApi, "is_audio_clip"));
    var filePath = null;
    var fileAccessible = false;
    var fileProbeError = null;

    if (isAudioClip === true) {
        var fileProbe = safe_getstring_with_error(clipApi, "file_path");
        filePath = fileProbe.value;
        fileProbeError = fileProbe.error;
        fileAccessible = filePath !== null && filePath !== "";
    }

    return {
        clip_slot_index: clipSlotIndex,
        clip_name: safe_getstring(clipApi, "name"),
        is_audio_clip: isAudioClip,
        is_midi_clip: safe_bool_or_null(safe_get(clipApi, "is_midi_clip")),
        source_file_path: filePath,
        source_file_accessible: fileAccessible,
        source_file_probe_error: fileProbeError,
        live_api_path: clipPath
    };
}

function make_observer(path, propertyName) {
    var api = new LiveAPI(function(args) {
        post("[ableton_bridge] observed change: " + path + " " + propertyName + " " + args + "\n");
        probe();
    }, path);
    api.property = propertyName;
    return api;
}

function make_api(path) {
    return new LiveAPI(null, path);
}

function is_valid_api(api) {
    return api && api.id && api.id !== 0;
}

function safe_getcount(api, childName) {
    try {
        if (!api) {
            return null;
        }
        return api.getcount(childName);
    } catch (error) {
        return null;
    }
}

function safe_get(api, propertyName) {
    try {
        if (!api) {
            return null;
        }
        return normalize_value(api.get(propertyName));
    } catch (error) {
        return null;
    }
}

function safe_getstring(api, propertyName) {
    return safe_getstring_with_error(api, propertyName).value;
}

function safe_getstring_with_error(api, propertyName) {
    try {
        if (!api) {
            return {
                value: null,
                error: "LiveAPI object unavailable"
            };
        }
        return {
            value: normalize_string(api.getstring(propertyName)),
            error: null
        };
    } catch (error) {
        return {
            value: null,
            error: String(error)
        };
    }
}

function normalize_value(value) {
    if (value instanceof Array && value.length === 1) {
        return value[0];
    }
    return value;
}

function normalize_string(value) {
    if (value === null || typeof value === "undefined") {
        return null;
    }
    if (value instanceof Array) {
        return value.join(" ");
    }
    return String(value);
}

function safe_bool(value) {
    return value === 1 || value === true || value === "1";
}

function safe_bool_or_null(value) {
    if (value === null || typeof value === "undefined") {
        return null;
    }
    return safe_bool(value);
}

function emit_json(payload) {
    var encoded = JSON.stringify(payload, null, 2);
    post(encoded + "\n");
    outlet(0, encoded);
}
