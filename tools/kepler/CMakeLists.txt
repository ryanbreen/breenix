cmake_minimum_required(VERSION 3.20)
project(breenix_kepler LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_C_STANDARD 99)
include(CheckIncludeFile)
check_include_file(endian.h HAVE_ENDIAN_H)
if(HAVE_ENDIAN_H)
  add_compile_definitions(HAVE_ENDIAN_H)
endif()
# Assertions remain enabled: the selected compiler relies on them for IR checks.
add_compile_options(-O2 -g0 -Werror -fno-fast-math -ffp-contract=off)
add_compile_definitions(_GNU_SOURCE HAVE___BUILTIN_EXPECT HAVE___BUILTIN_CLZ HAVE___BUILTIN_CLZLL HAVE___BUILTIN_CTZ HAVE___BUILTIN_CTZLL HAVE___BUILTIN_POPCOUNT HAVE___BUILTIN_POPCOUNTLL)
get_filename_component(ROOT ../.. ABSOLUTE BASE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
set(E "${ROOT}/vendor/kepler/envytools")
find_package(Python3 REQUIRED COMPONENTS Interpreter)
add_custom_target(verify-kepler-sources
  COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/assets.py" --verify-sources
  VERBATIM)
find_package(BISON 3.8.2 EXACT REQUIRED)
find_package(FLEX 2.6.4 EXACT REQUIRED)
BISON_TARGET(easm "${E}/easm/easm_parse.y" "${CMAKE_CURRENT_BINARY_DIR}/easm_parse.c" DEFINES_FILE "${CMAKE_CURRENT_BINARY_DIR}/easm_parse.h")
FLEX_TARGET(easm "${E}/easm/easm_lex.l" "${CMAKE_CURRENT_BINARY_DIR}/easm_lex.c" DEFINES_FILE "${CMAKE_CURRENT_BINARY_DIR}/easm_lex.h")
ADD_FLEX_BISON_DEPENDENCY(easm easm)
set(ENVY_SOURCES envydis/core.c envydis/core-as.c envydis/core-dis.c envydis/falcon.c envydis/gk110.c easm/easm.c easm/easm_print.c easm/easm_xfrm.c util/mask.c util/hash.c util/symtab.c util/colors.c util/yy.c util/astr.c util/aprintf.c util/vardata.c util/varinfo.c util/varselect.c util/file.c)
list(TRANSFORM ENVY_SOURCES PREPEND "${E}/")
add_library(envy STATIC ${ENVY_SOURCES} ${BISON_easm_OUTPUTS} ${FLEX_easm_OUTPUTS})
target_include_directories(envy PUBLIC "${E}/include" "${E}/envydis" "${CMAKE_CURRENT_BINARY_DIR}")
add_executable(envyas "${E}/envydis/envyas.c")
add_executable(envydis "${E}/envydis/envydis.c")
target_link_libraries(envyas PRIVATE envy)
target_link_libraries(envydis PRIVATE envy)
set(M "${ROOT}/vendor/kepler/mesa")
set(CG "${M}/src/nouveau/codegen")
set(CODEGEN nv50_ir.cpp nv50_ir_bb.cpp nv50_ir_build_util.cpp nv50_ir_graph.cpp nv50_ir_lowering_helper.cpp nv50_ir_peephole.cpp nv50_ir_print.cpp nv50_ir_ra.cpp nv50_ir_ssa.cpp nv50_ir_target.cpp nv50_ir_util.cpp nv50_ir_emit_gk110.cpp nv50_ir_emit_nvc0.cpp nv50_ir_lowering_nvc0.cpp nv50_ir_target_nvc0.cpp)
list(TRANSFORM CODEGEN PREPEND "${CG}/")
add_library(nouveau_codegen STATIC ${CODEGEN})
target_include_directories(nouveau_codegen PUBLIC "${CG}" "${M}/src" "${M}/include" "${M}/src/gallium/include")
add_executable(kepler-compile compile.cpp)
target_compile_options(kepler-compile PRIVATE -Wall -Wextra)
target_link_libraries(kepler-compile PRIVATE nouveau_codegen)
foreach(target envy envyas envydis nouveau_codegen kepler-compile)
  add_dependencies(${target} verify-kepler-sources)
endforeach()
find_program(KEPLER_CPP NAMES cpp-16 REQUIRED)
file(GLOB_RECURSE KEPLER_INPUTS CONFIGURE_DEPENDS
  "${ROOT}/vendor/kepler/*"
  "${ROOT}/kernel/src/gpu/virgl/pipelines/*.tgsi"
  "${ROOT}/kernel/src/gpu/virgl/profile/*.tgsi"
  "${CMAKE_CURRENT_SOURCE_DIR}/shaders/*.tgsi"
  "${ROOT}/tests/fixtures/gt710_shader_controls/*.tgsi")
add_custom_command(OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/assets/manifest.json"
  COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/assets.py"
          --tools "${CMAKE_CURRENT_BINARY_DIR}" --output "${CMAKE_CURRENT_BINARY_DIR}/assets"
          --cpp "${KEPLER_CPP}"
  DEPENDS envyas envydis kepler-compile ${KEPLER_INPUTS}
          "${CMAKE_CURRENT_SOURCE_DIR}/assets.py" "${ROOT}/libs/gpu-abi/src/lib.rs"
  VERBATIM)
add_custom_target(kepler-assets DEPENDS "${CMAKE_CURRENT_BINARY_DIR}/assets/manifest.json")

file(GENERATE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/toolchain.json"
  CONTENT "{\"c\":\"${CMAKE_C_COMPILER_ID} ${CMAKE_C_COMPILER_VERSION}\",\"cxx\":\"${CMAKE_CXX_COMPILER_ID} ${CMAKE_CXX_COMPILER_VERSION}\",\"bison\":\"${BISON_VERSION}\",\"flex\":\"${FLEX_VERSION}\",\"flags\":\"-O2 -g0 -Werror -fno-fast-math -ffp-contract=off; assertions enabled\"}\n")
