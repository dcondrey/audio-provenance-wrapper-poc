# Per-format install rules, included optionally from CMakeLists.txt after every plug-in target
# exists. Each format has its own install COMPONENT so users pick what to install:
#
#   cmake --install <build> --component vst3 --prefix <dir>
#   cmake --install <build> --component clap --prefix <dir>
#
# Components: vst3 au clap lv2 vst2 auv3 aax ladspa dssi daemon
# Only formats that were actually configured get rules (if(TARGET ...)), so a component that
# was not built is simply absent and `cmake --install --component <it>` installs nothing.
#
# Layout under the prefix, by platform:
#   Linux / other   lib/{vst3,clap,lv2,ladspa,dssi}/         (what scripts/install_linux.sh reads)
#   macOS           Library/Audio/Plug-Ins/{VST3,Components,CLAP,LV2,VST}/
#   Windows         {VST3,CLAP,LV2,VST2,AAX}/                (what packaging/windows/installer.iss reads)
#
# Plug-in bundles are copied verbatim from the build tree. `cmake --install` does not sign, so
# a macOS release must go through scripts/package_installer.sh + scripts/package_pkg.sh.

include(GNUInstallDirs)

set(_apw_target AudioProvenanceCapture)

if(APPLE)
    set(_apw_dir_vst3 "Library/Audio/Plug-Ins/VST3")
    set(_apw_dir_au "Library/Audio/Plug-Ins/Components")
    set(_apw_dir_clap "Library/Audio/Plug-Ins/CLAP")
    set(_apw_dir_lv2 "Library/Audio/Plug-Ins/LV2")
    set(_apw_dir_vst2 "Library/Audio/Plug-Ins/VST")
    set(_apw_dir_auv3 "Applications")
    set(_apw_dir_aax "Library/Application Support/Avid/Audio/Plug-Ins")
elseif(WIN32)
    set(_apw_dir_vst3 "VST3")
    set(_apw_dir_clap "CLAP")
    set(_apw_dir_lv2 "LV2")
    set(_apw_dir_vst2 "VST2")
    set(_apw_dir_aax "AAX")
else()
    set(_apw_dir_vst3 "${CMAKE_INSTALL_LIBDIR}/vst3")
    set(_apw_dir_clap "${CMAKE_INSTALL_LIBDIR}/clap")
    set(_apw_dir_lv2 "${CMAKE_INSTALL_LIBDIR}/lv2")
    set(_apw_dir_vst2 "${CMAKE_INSTALL_LIBDIR}/vst")
    set(_apw_dir_aax "${CMAKE_INSTALL_LIBDIR}/aax")
    set(_apw_dir_ladspa "${CMAKE_INSTALL_LIBDIR}/ladspa")
    set(_apw_dir_dssi "${CMAKE_INSTALL_LIBDIR}/dssi")
endif()

# JUCE records each format's artefact (bundle directory or file) in this property.
# _apw_install_artefact(<component> <target> <destination> <directory|file>)
function(_apw_install_artefact component target destination kind)
    if(NOT TARGET "${target}")
        return()
    endif()
    get_target_property(_artefact "${target}" JUCE_PLUGIN_ARTEFACT_FILE)
    if(NOT _artefact)
        message(WARNING "cmake/install.cmake: ${target} has no JUCE_PLUGIN_ARTEFACT_FILE; component '${component}' will install nothing")
        return()
    endif()
    if(kind STREQUAL "directory")
        install(DIRECTORY "${_artefact}"
                DESTINATION "${destination}"
                USE_SOURCE_PERMISSIONS
                COMPONENT ${component})
    else()
        install(FILES "${_artefact}"
                DESTINATION "${destination}"
                COMPONENT ${component})
    endif()
endfunction()

# Bundle-shaped on Apple for every format; VST3, LV2 and AAX are directories on every OS.
if(APPLE)
    set(_apw_kind_default directory)
else()
    set(_apw_kind_default file)
endif()

_apw_install_artefact(vst3 ${_apw_target}_VST3 "${_apw_dir_vst3}" directory)
_apw_install_artefact(lv2 ${_apw_target}_LV2 "${_apw_dir_lv2}" directory)
_apw_install_artefact(aax ${_apw_target}_AAX "${_apw_dir_aax}" directory)
_apw_install_artefact(clap ${_apw_target}_CLAP "${_apw_dir_clap}" ${_apw_kind_default})
_apw_install_artefact(vst2 ${_apw_target}_VST "${_apw_dir_vst2}" ${_apw_kind_default})
if(APPLE)
    _apw_install_artefact(au ${_apw_target}_AU "${_apw_dir_au}" directory)
    # An AUv3 appex only registers from its container app, which is built with the Standalone
    # target; without one there is nothing installable.
    if(TARGET ${_apw_target}_Standalone)
        _apw_install_artefact(auv3 ${_apw_target}_Standalone "${_apw_dir_auv3}" directory)
    endif()
endif()

# LADSPA / DSSI come from cmake/ladspa_dssi.cmake, which may not exist. They are shared
# libraries, so any of the plausible target names is installed as a library.
foreach(_apw_fmt ladspa dssi)
    string(TOUPPER ${_apw_fmt} _apw_fmt_upper)
    foreach(_apw_candidate
            ${_apw_target}_${_apw_fmt_upper}
            ${_apw_target}_${_apw_fmt}
            apw_${_apw_fmt}
            apw_${_apw_fmt}_plugin)
        if(TARGET ${_apw_candidate} AND DEFINED _apw_dir_${_apw_fmt})
            install(TARGETS ${_apw_candidate}
                    LIBRARY DESTINATION "${_apw_dir_${_apw_fmt}}" COMPONENT ${_apw_fmt}
                    RUNTIME DESTINATION "${_apw_dir_${_apw_fmt}}" COMPONENT ${_apw_fmt})
            break()
        endif()
    endforeach()
endforeach()

# The frozen daemon is produced by scripts/package_installer.sh (PyInstaller), not by CMake.
# Point APW_DAEMON_DIR at a directory containing it to make it installable as its own component.
set(APW_DAEMON_DIR "" CACHE PATH "Frozen apw-daemon directory to install as component 'daemon'")
if(APW_DAEMON_DIR AND EXISTS "${APW_DAEMON_DIR}")
    install(DIRECTORY "${APW_DAEMON_DIR}/"
            DESTINATION "${CMAKE_INSTALL_DATADIR}/audio-provenance-capture/apw-daemon"
            USE_SOURCE_PERMISSIONS
            COMPONENT daemon)
endif()
