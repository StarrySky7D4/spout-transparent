cmake_minimum_required(VERSION 3.20)
project(spout_c_example LANGUAGES C)

set(SPT_SDK_ROOT "" CACHE PATH "Directory produced by build-sdk.ps1")
if(NOT EXISTS "${SPT_SDK_ROOT}/include/spout_transparent.h")
    message(FATAL_ERROR "Set SPT_SDK_ROOT to the generated C ABI SDK directory")
endif()

add_library(spout_c SHARED IMPORTED)
set_target_properties(spout_c PROPERTIES
    IMPORTED_IMPLIB "${SPT_SDK_ROOT}/lib/spout_transparent_c.dll.lib"
    IMPORTED_LOCATION "${SPT_SDK_ROOT}/bin/spout_transparent_c.dll"
    INTERFACE_INCLUDE_DIRECTORIES "${SPT_SDK_ROOT}/include")

add_executable(spout_c_example list_senders.c)
set_target_properties(spout_c_example PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED YES)
target_link_libraries(spout_c_example PRIVATE spout_c)
add_custom_command(TARGET spout_c_example POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "$<TARGET_FILE:spout_c>" "$<TARGET_FILE_DIR:spout_c_example>")
