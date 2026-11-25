# SPDX-License-Identifier: MIT
#
# A CMake front end over cargo, for IDEs (VS Code with CMake Tools: .vscode/). Cargo stays the
# build system and nothing here compiles anything itself: the project has no C (LANGUAGES NONE).
# The command lines in docs/DEVELOPMENT.md work the same without CMake.
#
#   cmake --preset release && cmake --build --preset release            lift if needed, build
#   cmake --build --preset release --target lift                        lift again
#   cmake --build --preset release --target run                         play
#   ctest --preset release                                              the checks
#
# Targets: technomage (default; the lifted code is lifted first when generated/ has none),
# lift, recomp (the recompiler), inputs (download and extract the media), run, web, and the
# checks audit, fmt, clippy, cargo-test (also as CTest tests). The game and the recompiler are
# put into <build dir>/bin/ too, so a debugger finds the active preset's build in one place.
cmake_minimum_required(VERSION 3.25)

file(READ "${CMAKE_CURRENT_SOURCE_DIR}/VERSION" RC_VERSION)
string(STRIP "${RC_VERSION}" RC_VERSION)
project(technomagerecomp VERSION ${RC_VERSION} LANGUAGES NONE)

find_program(CARGO cargo HINTS "$ENV{CARGO_HOME}/bin" "$ENV{HOME}/.cargo/bin" "$ENV{USERPROFILE}/.cargo/bin"
             REQUIRED)

set(RC_CARGO_PROFILE release CACHE STRING "Cargo profile of the game (dev, release, dist)")
set_property(CACHE RC_CARGO_PROFILE PROPERTY STRINGS dev release dist)
set(RC_LANGUAGE de CACHE STRING "Language of the media that 'inputs' downloads (de, en)")
set_property(CACHE RC_LANGUAGE PROPERTY STRINGS de en)
if(RC_LANGUAGE STREQUAL "en")
  set(_variant cd-en)
else()
  set(_variant cd-de)
endif()
set(RC_GAME_DIR "${CMAKE_SOURCE_DIR}/orig/technomage/${_variant}" CACHE PATH
    "The installed game: what 'lift' lifts and 'run' plays")
option(RC_NO_GENERATED "Build the shell without lifted code (the converter's template)" OFF)
set(RC_CARGO_ARGS "" CACHE STRING "Extra arguments for cargo build (a ;-list, e.g. --features;x)")
# The template has its own, or each build would undo the other's (RC_NO_GENERATED changes the shell).
set(RC_CARGO_TARGET_DIR "${CMAKE_SOURCE_DIR}/target" CACHE PATH "Cargo's target directory for the game")

set(_src "${CMAKE_SOURCE_DIR}")
set(_bin "${CMAKE_BINARY_DIR}/bin")
if(CMAKE_HOST_WIN32)
  set(_exe ".exe")
else()
  set(_exe "")
endif()
if(RC_CARGO_PROFILE STREQUAL "dev")
  set(_profile_dir debug)
else()
  set(_profile_dir "${RC_CARGO_PROFILE}")
endif()
set(_game "${RC_CARGO_TARGET_DIR}/${_profile_dir}/technomage${_exe}")
# The lifter is always optimised: a dev build lifts too slowly. Its release profile keeps line
# tables, enough to step through it.
set(_recomp "${_src}/target/release/recomp${_exe}")
set(_env "${CMAKE_COMMAND}" -E env "CARGO_TARGET_DIR=${RC_CARGO_TARGET_DIR}")
if(RC_NO_GENERATED)
  list(APPEND _env RC_NO_GENERATED=1)
else()
  list(APPEND _env --unset=RC_NO_GENERATED)
endif()

# Put a cargo output into <build dir>/bin/: a link where links are cheap, a copy on Windows
# (whose executables find their .pdb by absolute path, so the copy debugs the same).
function(rc_publish target file)
  get_filename_component(_name "${file}" NAME)
  if(CMAKE_HOST_WIN32)
    set(_cmd copy_if_different "${file}" "${_bin}/${_name}")
  else()
    set(_cmd create_symlink "${file}" "${_bin}/${_name}")
  endif()
  add_custom_command(TARGET ${target} POST_BUILD
                     COMMAND "${CMAKE_COMMAND}" -E make_directory "${_bin}"
                     COMMAND "${CMAKE_COMMAND}" -E ${_cmd}
                     VERBATIM)
endfunction()

# --- The recompiler and the lift -------------------------------------------------------------

add_custom_target(recomp
  COMMAND "${CARGO}" build --release -p rc-cli
  WORKING_DIRECTORY "${_src}"
  BYPRODUCTS "${_recomp}"
  COMMENT "cargo build --release -p rc-cli"
  USES_TERMINAL VERBATIM)
rc_publish(recomp "${_recomp}")

# The lifter writes only the files that changed, so lifting again rebuilds only what changed.
set(_lift_cmd "${_recomp}" lift "${RC_GAME_DIR}")
add_custom_target(lift
  COMMAND ${_lift_cmd}
  WORKING_DIRECTORY "${_src}"
  COMMENT "recomp lift ${RC_GAME_DIR}"
  USES_TERMINAL VERBATIM)
add_dependencies(lift recomp)

# The default build lifts only when there is no lifted code yet (or the executable is newer).
set(_set_rs "${_src}/generated/technomage/set.rs")
set(_lift_deps recomp)
foreach(_f TechnoMage.exe binkw32.dll)
  if(EXISTS "${RC_GAME_DIR}/${_f}")
    list(APPEND _lift_deps "${RC_GAME_DIR}/${_f}")
  endif()
endforeach()
if(NOT EXISTS "${RC_GAME_DIR}")
  message(WARNING "RC_GAME_DIR (${RC_GAME_DIR}) does not exist: build the 'inputs' target, or set "
                  "RC_GAME_DIR to an installed game, then configure again. Until then the game builds "
                  "without lifted code.")
endif()
add_custom_command(OUTPUT "${_set_rs}"
  COMMAND ${_lift_cmd}
  DEPENDS ${_lift_deps}
  WORKING_DIRECTORY "${_src}"
  COMMENT "recomp lift ${RC_GAME_DIR} (no lifted code yet)"
  USES_TERMINAL VERBATIM)
add_custom_target(lifted DEPENDS "${_set_rs}")

# The media from archive.org, checked by hash, unpacked to orig/technomage/<variant>/.
if(RC_LANGUAGE STREQUAL "en")
  set(_medium "${_src}/orig/TECHNOMAGE.mdf")
else()
  set(_medium "${_src}/orig/2004-03.BIN")
endif()
add_custom_target(inputs
  COMMAND "${_recomp}" download --dest orig technomage ${RC_LANGUAGE}
  COMMAND "${_recomp}" extract "${_medium}"
  WORKING_DIRECTORY "${_src}"
  COMMENT "recomp download and extract (${RC_LANGUAGE})"
  USES_TERMINAL VERBATIM)
add_dependencies(inputs recomp)

# --- The game --------------------------------------------------------------------------------

add_custom_target(technomage ALL
  COMMAND ${_env} "${CARGO}" build --profile ${RC_CARGO_PROFILE} -p technomage ${RC_CARGO_ARGS}
  WORKING_DIRECTORY "${_src}"
  BYPRODUCTS "${_game}"
  COMMENT "cargo build --profile ${RC_CARGO_PROFILE} -p technomage"
  USES_TERMINAL VERBATIM)
if(NOT RC_NO_GENERATED AND EXISTS "${RC_GAME_DIR}")
  add_dependencies(technomage lifted)
endif()
rc_publish(technomage "${_game}")

add_custom_target(run
  COMMAND "${_game}" --game-dir "${RC_GAME_DIR}"
  WORKING_DIRECTORY "${_src}"
  COMMENT "technomage --game-dir ${RC_GAME_DIR}"
  USES_TERMINAL VERBATIM)
add_dependencies(run technomage)

# The converter site with the browser player (tools/build-web, needs bash and wasm-bindgen).
find_program(BASH bash)
if(BASH)
  add_custom_target(web
    COMMAND "${BASH}" tools/build-web out/site
    WORKING_DIRECTORY "${_src}"
    COMMENT "tools/build-web out/site"
    USES_TERMINAL VERBATIM)
endif()

# --- Checks (docs/DEVELOPMENT.md) -----------------------------------------------------------

enable_testing()
# A check as a target (to run it on its own) and as a CTest test (the IDE's test view).
function(rc_check name)
  add_custom_target(${name} COMMAND ${ARGN} WORKING_DIRECTORY "${_src}" USES_TERMINAL VERBATIM)
  add_test(NAME ${name} COMMAND ${ARGN} WORKING_DIRECTORY "${_src}")
  set_tests_properties(${name} PROPERTIES TIMEOUT 3600)
endfunction()
if(BASH)
  rc_check(audit "${BASH}" tools/audit.sh)
endif()
rc_check(fmt "${CARGO}" fmt --all --check)
rc_check(clippy "${CARGO}" clippy --all-targets -- -D warnings)
rc_check(cargo-test "${CARGO}" test)

message(STATUS "TechnoMageRecomp ${RC_VERSION}: cargo profile ${RC_CARGO_PROFILE}, game ${RC_GAME_DIR}")
