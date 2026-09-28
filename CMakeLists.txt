cmake_minimum_required(VERSION 3.22)

project(AudioProvenance
    VERSION 0.9.0
    LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(APPLE AND (NOT DEFINED CMAKE_OSX_DEPLOYMENT_TARGET OR CMAKE_OSX_DEPLOYMENT_TARGET STREQUAL ""))
    set(CMAKE_OSX_DEPLOYMENT_TARGET "11.0" CACHE STRING "Minimum macOS deployment target" FORCE)
endif()

# REQUIRED: redistributable builds must load in Intel and Rosetta hosts, not just arm64.
if(APPLE AND (NOT DEFINED CMAKE_OSX_ARCHITECTURES OR CMAKE_OSX_ARCHITECTURES STREQUAL ""))
    set(CMAKE_OSX_ARCHITECTURES "arm64;x86_64" CACHE STRING "Universal binary slices" FORCE)
endif()

set(APW_JUCE_DIR "" CACHE PATH "Path to a local JUCE checkout")
set(APW_JUCE_TAG "8.0.15" CACHE STRING "Pinned JUCE release used when no local checkout is supplied")
option(APW_FETCH_JUCE "Fetch the pinned JUCE release when JUCE is not installed" ON)
set(APW_HEADLESS_SOAK_SECONDS "0" CACHE STRING
    "Wall-clock duration for the deterministic AudioProcessorPlayer soak CTest (0 disables soak phase)")

if(APW_JUCE_DIR)
    add_subdirectory("${APW_JUCE_DIR}" "${CMAKE_BINARY_DIR}/JUCE")
else()
    find_package(JUCE CONFIG QUIET)

    if(NOT JUCE_FOUND)
        if(APW_FETCH_JUCE)
            include(FetchContent)
            FetchContent_Declare(
                JUCE
                GIT_REPOSITORY https://github.com/juce-framework/JUCE.git
                GIT_TAG ${APW_JUCE_TAG}
                GIT_SHALLOW TRUE)
            FetchContent_MakeAvailable(JUCE)
        else()
            message(FATAL_ERROR "JUCE not found. Set APW_JUCE_DIR or enable APW_FETCH_JUCE.")
        endif()
    endif()
endif()

# Plug-in formats are per platform. AU is Apple-only (JUCE also disables it
# elsewhere). VST3 builds everywhere. LV2 is enabled by default off Apple and
# can be requested on Apple with -DAPW_BUILD_LV2=ON. AAX needs Avid's SDK and
# PACE signing and is deliberately not built. Standalone is not built: the
# plug-in observes a host and a standalone shell has none.
set(APW_PLUGIN_FORMATS VST3)
if(APPLE)
    list(APPEND APW_PLUGIN_FORMATS AU)
    option(APW_BUILD_LV2 "Also build the LV2 format" OFF)
else()
    option(APW_BUILD_LV2 "Also build the LV2 format" ON)
endif()
if(APW_BUILD_LV2)
    list(APPEND APW_PLUGIN_FORMATS LV2)
endif()
message(STATUS "AudioProvenanceCapture plug-in formats: ${APW_PLUGIN_FORMATS}")

juce_add_plugin(AudioProvenanceCapture
    COMPANY_NAME "Audio Provenance POC"
    BUNDLE_ID "com.audioprovenance.capture"
    IS_SYNTH FALSE
    NEEDS_MIDI_INPUT TRUE
    NEEDS_MIDI_OUTPUT FALSE
    IS_MIDI_EFFECT FALSE
    EDITOR_WANTS_KEYBOARD_FOCUS FALSE
    COPY_PLUGIN_AFTER_BUILD FALSE
    PLUGIN_MANUFACTURER_CODE ApPr
    PLUGIN_CODE ApCa
    FORMATS ${APW_PLUGIN_FORMATS}
    LV2URI "urn:audioprovenance:capture"
    PRODUCT_NAME "Audio Provenance Capture")

target_sources(AudioProvenanceCapture
    PRIVATE
        src/AudioObserver.cpp
        src/AcknowledgementLogic.cpp
        src/EventEmitter.cpp
        src/PluginEditor.cpp
        src/PluginProcessor.cpp)

target_include_directories(AudioProvenanceCapture
    PRIVATE
        src)

target_compile_definitions(AudioProvenanceCapture
    PUBLIC
        JUCE_WEB_BROWSER=0
        JUCE_USE_CURL=0
        JUCE_VST3_CAN_REPLACE_VST2=0)

target_link_libraries(AudioProvenanceCapture
    PRIVATE
        juce::juce_audio_processors
        juce::juce_cryptography
        juce::juce_dsp
        juce::juce_gui_basics
    PUBLIC
        juce::juce_recommended_config_flags
        juce::juce_recommended_lto_flags
        juce::juce_recommended_warning_flags)

include(CTest)

if(BUILD_TESTING)
    find_package(Python3 QUIET COMPONENTS Interpreter)
    if(Python3_Interpreter_FOUND)
        set(APW_PYTHON "${Python3_EXECUTABLE}")
    else()
        set(APW_PYTHON python3)
    endif()

    juce_add_console_app(AudioProvenanceCaptureTests
        PRODUCT_NAME "Audio Provenance Capture Tests")

    target_sources(AudioProvenanceCaptureTests
        PRIVATE
            tests/cpp/PluginRealtimeTests.cpp
            src/AudioObserver.cpp
            src/AcknowledgementLogic.cpp
            src/EventEmitter.cpp
            src/PluginEditor.cpp
            src/PluginProcessor.cpp)

    target_include_directories(AudioProvenanceCaptureTests PRIVATE src)

    target_compile_definitions(AudioProvenanceCaptureTests
        PRIVATE
            JucePlugin_Name="Audio Provenance Capture"
            JUCE_WEB_BROWSER=0
            JUCE_USE_CURL=0
            JUCE_VST3_CAN_REPLACE_VST2=0)

    target_link_libraries(AudioProvenanceCaptureTests
        PRIVATE
            juce::juce_audio_processors
            juce::juce_cryptography
            juce::juce_dsp
            juce::juce_gui_basics
            juce::juce_recommended_config_flags
            juce::juce_recommended_warning_flags)

    add_test(NAME plugin_realtime_lifecycle COMMAND AudioProvenanceCaptureTests)

    # The lifecycle harness replaces global operator new/delete and sizes blocks
    # with malloc_size (macOS) or malloc_usable_size (Linux); it has no Windows
    # allocator port, so it is not built there.
    if(APPLE OR CMAKE_SYSTEM_NAME STREQUAL "Linux")
        juce_add_console_app(AudioProvenanceHeadlessHostTests
            PRODUCT_NAME "Audio Provenance Headless DAW Lifecycle Tests")

        target_sources(AudioProvenanceHeadlessHostTests
            PRIVATE
                tests/cpp/HeadlessDawLifecycleTests.cpp
                src/AudioObserver.cpp
                src/AcknowledgementLogic.cpp
                src/EventEmitter.cpp
                src/PluginEditor.cpp
                src/PluginProcessor.cpp)

        target_include_directories(AudioProvenanceHeadlessHostTests PRIVATE src)

        target_compile_definitions(AudioProvenanceHeadlessHostTests
            PRIVATE
                JucePlugin_Name="Audio Provenance Capture"
                JUCE_WEB_BROWSER=0
                JUCE_USE_CURL=0
                JUCE_VST3_CAN_REPLACE_VST2=0)

        target_link_libraries(AudioProvenanceHeadlessHostTests
            PRIVATE
                juce::juce_audio_processors
                juce::juce_audio_utils
                juce::juce_cryptography
                juce::juce_dsp
                juce::juce_gui_basics
                juce::juce_recommended_config_flags
                juce::juce_recommended_warning_flags)

        add_test(
            NAME plugin_headless_daw_matrix
            COMMAND AudioProvenanceHeadlessHostTests
                    --soak-seconds ${APW_HEADLESS_SOAK_SECONDS}
                    --seed 4706347393385544497)
        set_tests_properties(plugin_headless_daw_matrix PROPERTIES TIMEOUT 33000)
    endif()

    add_test(
        NAME plugin_realtime_source_audit
        COMMAND ${CMAKE_COMMAND} -E env PYTHONDONTWRITEBYTECODE=1
                ${APW_PYTHON} ${CMAKE_SOURCE_DIR}/scripts/audit_plugin_realtime.py)
endif()
