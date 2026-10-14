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
endfunction()

cmake_language(DEFER CALL parakit_correct_native_session)
