These are unfathomable bullshit hacks.

Basically what we're doing here is implementing our own "copy" of the cxa exception handling functions. We then function patch the static libstdc++ functions to detour to our own functions.