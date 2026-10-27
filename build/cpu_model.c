/* zig's runtime has no __cpu_model, libgcc's table of CPU features that
   __builtin_cpu_supports reads (UnRAR asks it for SSE / AES). All zero
   means "no extra features": UnRAR takes its plain code paths. Linked
   only into builds made with zig (the Linux glibc-2.28 packages). */
struct dc_processor_model {
    unsigned int vendor;
    unsigned int type;
    unsigned int subtype;
    unsigned int features[1];
};
struct dc_processor_model __cpu_model = {0, 0, 0, {0}};
unsigned int __cpu_features2[4] = {0, 0, 0, 0};
int __cpu_indicator_init(void) { return 0; }
