# Script mode (cmake -P): run `render` and fail unless it wrote a PNG bigger
# than MIN_BYTES. A render exits 0 even when every feature lands outside the
# viewport (e.g. a reprojection silently gone wrong), so size is the cheap
# tell that something was actually drawn: a blank frame compresses to ~1-2 KB.
#
# Optional FORBID is a regex that must not appear in render's output -- for
# failures mapnik only logs (e.g. "could not create BreakIterator") instead of
# exiting non-zero.
#
#   cmake -DRENDER=<exe> -DOUT=<png> -DMIN_BYTES=<n> -DARGS=<;-list>
#         [-DFORBID=<regex>] -P check-render.cmake
execute_process(COMMAND ${RENDER} ${ARGS}
    RESULT_VARIABLE rc OUTPUT_VARIABLE out ERROR_VARIABLE err)
message("${out}${err}")
if(NOT rc EQUAL 0)
    message(FATAL_ERROR "render exited ${rc}")
endif()
if(FORBID AND "${out}${err}" MATCHES "${FORBID}")
    message(FATAL_ERROR "render output matched forbidden pattern: ${FORBID}")
endif()
file(SIZE "${OUT}" size)
if(size LESS MIN_BYTES)
    message(FATAL_ERROR "${OUT} is ${size} bytes (< ${MIN_BYTES}): nothing was drawn")
endif()
message(STATUS "${OUT}: ${size} bytes")
