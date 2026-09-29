# LADSPA 1.1 and DSSI 1.0 shims around the capture processor. Included
# optionally from the top-level CMakeLists.txt after JUCE is available.
#
#   -DAPW_BUILD_LADSPA=ON   builds audio_provenance_capture_ladspa.so
#   -DAPW_BUILD_DSSI=ON     builds audio_provenance_capture_dssi.so
#
# Both are plain shared modules (Linux first; they also build on macOS so the
# test hosts can dlopen them). See docs/LADSPA_DSSI.md.

option(APW_BUILD_LADSPA "Build the LADSPA plug-in module" OFF)
option(APW_BUILD_DSSI "Build the DSSI plug-in module" OFF)
include(CTest)

if(APW_BUILD_LADSPA OR APW_BUILD_DSSI)
    set(APW_SHIM_DIR "${CMAKE_CURRENT_LIST_DIR}/..")

    # ALSA is only needed for snd_seq_event_t. Use the system header when there
    # is one, otherwise the layout-compatible stand-in under src/dssi/compat.
    find_path(APW_ALSA_INCLUDE_DIR alsa/seq_event.h)
    if(APW_ALSA_INCLUDE_DIR)
        set(APW_SHIM_ALSA_INCLUDE "${APW_ALSA_INCLUDE_DIR}")
    else()
        set(APW_SHIM_ALSA_INCLUDE "${APW_SHIM_DIR}/src/dssi/compat")
    endif()

    # The processor and its observation pipeline, compiled once for both modules.
    add_library(apw_shim_core STATIC
        "${APW_SHIM_DIR}/src/ladspa/CaptureInstance.cpp"
        "${APW_SHIM_DIR}/src/AudioObserver.cpp"
        "${APW_SHIM_DIR}/src/AcknowledgementLogic.cpp"
        "${APW_SHIM_DIR}/src/EventEmitter.cpp"
        "${APW_SHIM_DIR}/src/PluginEditor.cpp"
        "${APW_SHIM_DIR}/src/PluginProcessor.cpp")
    # JUCE modules are INTERFACE libraries that carry their sources, so they
    # are linked PRIVATE: only this archive compiles JUCE, and the entry points
    # and test hosts (which include no JUCE header) just link it.
    target_include_directories(apw_shim_core
        PRIVATE "${APW_SHIM_DIR}/src"
        PUBLIC "${APW_SHIM_DIR}/src/ladspa" "${APW_SHIM_ALSA_INCLUDE}")
    target_compile_definitions(apw_shim_core
        PRIVATE
            JucePlugin_Name="Audio Provenance Capture"
            JUCE_WEB_BROWSER=0
            JUCE_USE_CURL=0
            JUCE_VST3_CAN_REPLACE_VST2=0
            JUCE_STANDALONE_APPLICATION=0)
    target_link_libraries(apw_shim_core
        PRIVATE
            juce::juce_audio_processors
            juce::juce_cryptography
            juce::juce_dsp
            juce::juce_gui_basics
            juce::juce_recommended_config_flags
            juce::juce_recommended_warning_flags)
    set_target_properties(apw_shim_core PROPERTIES
        POSITION_INDEPENDENT_CODE ON
        CXX_VISIBILITY_PRESET hidden
        C_VISIBILITY_PRESET hidden
        VISIBILITY_INLINES_HIDDEN ON)

    function(apw_add_shim_module target output entry_source)
        add_library(${target} MODULE "${entry_source}")
        target_link_libraries(${target} PRIVATE apw_shim_core)
        target_include_directories(${target} PRIVATE "${APW_SHIM_DIR}/src/dssi")
        set_target_properties(${target} PROPERTIES
            PREFIX ""
            SUFFIX ".so"
            OUTPUT_NAME "${output}"
            CXX_VISIBILITY_PRESET hidden
            C_VISIBILITY_PRESET hidden
            VISIBILITY_INLINES_HIDDEN ON)
        if(NOT APPLE)
            # Keep the statically linked JUCE (and its bundled C code) private.
            target_link_options(${target} PRIVATE "LINKER:--exclude-libs,ALL")
        endif()
    endfunction()

    if(APW_BUILD_LADSPA)
        apw_add_shim_module(apw_ladspa audio_provenance_capture_ladspa
            "${APW_SHIM_DIR}/src/ladspa/LadspaEntry.cpp")
    endif()
    if(APW_BUILD_DSSI)
        apw_add_shim_module(apw_dssi audio_provenance_capture_dssi
            "${APW_SHIM_DIR}/src/dssi/DssiEntry.cpp")
    endif()

    if(BUILD_TESTING)
        # Every test binds the daemon port, so none may overlap another.
        if(APW_BUILD_LADSPA)
            # Loads the real module the way a host would.
            add_executable(apw_ladspa_host_tests "${APW_SHIM_DIR}/tests/cpp/LadspaHostTests.cpp")
            target_link_libraries(apw_ladspa_host_tests PRIVATE ${CMAKE_DL_LIBS})
            target_include_directories(apw_ladspa_host_tests PRIVATE "${APW_SHIM_DIR}/src/ladspa")
            add_dependencies(apw_ladspa_host_tests apw_ladspa)
            add_test(NAME ladspa_module_host
                COMMAND apw_ladspa_host_tests $<TARGET_FILE:apw_ladspa>)

            # Same checks against the entry points linked in-process, plus an
            # operator-new counter on the run thread.
            add_executable(apw_ladspa_realtime_tests
                "${APW_SHIM_DIR}/tests/cpp/LadspaHostTests.cpp"
                "${APW_SHIM_DIR}/src/ladspa/LadspaEntry.cpp")
            target_compile_definitions(apw_ladspa_realtime_tests PRIVATE APW_STATIC_ENTRY=1)
            target_link_libraries(apw_ladspa_realtime_tests PRIVATE apw_shim_core)
            add_test(NAME ladspa_realtime_static COMMAND apw_ladspa_realtime_tests)
            set_tests_properties(ladspa_module_host ladspa_realtime_static PROPERTIES RUN_SERIAL TRUE)
        endif()
        if(APW_BUILD_DSSI)
            add_executable(apw_dssi_host_tests "${APW_SHIM_DIR}/tests/cpp/DssiHostTests.cpp")
            target_link_libraries(apw_dssi_host_tests PRIVATE ${CMAKE_DL_LIBS})
            target_include_directories(apw_dssi_host_tests PRIVATE
                "${APW_SHIM_DIR}/src/dssi" "${APW_SHIM_DIR}/src/ladspa" "${APW_SHIM_ALSA_INCLUDE}")
            add_dependencies(apw_dssi_host_tests apw_dssi)
            add_test(NAME dssi_module_host
                COMMAND apw_dssi_host_tests $<TARGET_FILE:apw_dssi>)

            add_executable(apw_dssi_realtime_tests
                "${APW_SHIM_DIR}/tests/cpp/DssiHostTests.cpp"
                "${APW_SHIM_DIR}/src/dssi/DssiEntry.cpp")
            target_compile_definitions(apw_dssi_realtime_tests PRIVATE APW_STATIC_ENTRY=1)
            target_include_directories(apw_dssi_realtime_tests PRIVATE "${APW_SHIM_DIR}/src/dssi")
            target_link_libraries(apw_dssi_realtime_tests PRIVATE apw_shim_core)
            add_test(NAME dssi_realtime_static COMMAND apw_dssi_realtime_tests)
            set_tests_properties(dssi_module_host dssi_realtime_static PROPERTIES RUN_SERIAL TRUE)
        endif()
    endif()
endif()
