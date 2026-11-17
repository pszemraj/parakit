# Project hook: substitute the two build-directory source copies after all
# CrispASR targets have been declared, leaving the vendor tree unchanged.
if(CMAKE_VERSION VERSION_LESS 3.19)
    message(FATAL_ERROR "Parakit's bundled CrispASR corrections require CMake 3.19 or newer")
endif()

function(parakit_correct_native_session)
    set_source_files_properties(
        "${PROJECT_SOURCE_DIR}/src/crispasr_c_api.cpp"
        "${PROJECT_SOURCE_DIR}/src/parakeet_orchestrate.cpp"
        DIRECTORY "${PROJECT_SOURCE_DIR}/src"
        PROPERTIES HEADER_FILE_ONLY TRUE
    )
    target_sources(crispasr-lib PRIVATE "${PARAKIT_CRISPASR_C_API_SOURCE}")
    target_sources(parakeet PRIVATE "${PARAKIT_PARAKEET_SOURCE}")
    target_include_directories(crispasr-lib PRIVATE
        "${PROJECT_SOURCE_DIR}/src"
        "${PARAKIT_NATIVE_CORRECTNESS_INCLUDE}"
    )

    # Ask CMake to resolve the complete consumer link line. This target is
    # described by the File API but is never compiled or installed.
    file(WRITE "${CMAKE_BINARY_DIR}/parakit-link.cpp" "int main() { return 0; }\n")
    add_executable(parakit-native-link EXCLUDE_FROM_ALL "${CMAKE_BINARY_DIR}/parakit-link.cpp")
    target_link_libraries(parakit-native-link PRIVATE crispasr-lib ggml)
    # A Rust linker driver does not add the C++ compiler's implicit runtime.
    file(GENERATE OUTPUT "${CMAKE_BINARY_DIR}/parakit-cxx-runtime.txt"
        CONTENT "$<JOIN:${CMAKE_CXX_IMPLICIT_LINK_LIBRARIES},\n>\n")
endfunction()

cmake_language(DEFER CALL parakit_correct_native_session)
