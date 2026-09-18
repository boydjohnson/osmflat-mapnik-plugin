# Script mode (cmake -P): run `render` and fail unless it wrote a PNG bigger
# than MIN_BYTES. A render exits 0 even when every feature lands outside the
# viewport (e.g. a reprojection silently gone wrong), so size is the cheap
# tell that something was actually drawn: a blank frame compresses to ~1-2 KB.
#
#   cmake -DRENDER=<exe> -DOUT=<png> -DMIN_BYTES=<n> -DARGS=<;-list> -P check-render.cmake
execute_process(COMMAND ${RENDER} ${ARGS} RESULT_VARIABLE rc)
if(NOT rc EQUAL 0)
    message(FATAL_ERROR "render exited ${rc}")
endif()
file(SIZE "${OUT}" size)
if(size LESS MIN_BYTES)
    message(FATAL_ERROR "${OUT} is ${size} bytes (< ${MIN_BYTES}): nothing was drawn")
endif()
message(STATUS "${OUT}: ${size} bytes")
