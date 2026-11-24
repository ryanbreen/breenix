/* The probe's foreign frame and scalar callbacks exercise the target C ABI. */
void rust_host_c_frame(void (*callback)(void)) {
    volatile int frame = 1;
    callback();
    frame = 2;
    (void)frame;
}
double rust_host_c_double(double value, double (*callback)(double)) { return callback(value); }
float rust_host_c_float(float value, float (*callback)(float)) { return callback(value); }
