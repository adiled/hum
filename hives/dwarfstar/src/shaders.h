/* dwarfstar shaders: point the engine at its Metal kernel sources.
 *
 * ds4 compiles its Metal kernels at runtime from .metal files, found next
 * to the binary or wherever DS4_METAL_*_SOURCE points. The build copies
 * them beside the bee, and this sets every override to an absolute path,
 * so the bee works from any working directory, as it must when orchd
 * starts it.
 */

#ifndef DWARFSTAR_SHADERS_H
#define DWARFSTAR_SHADERS_H

int dwarfstar_shaders_point_at(const char *dir);

#endif
