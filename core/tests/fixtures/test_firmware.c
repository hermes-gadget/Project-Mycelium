static int setup_calls;
static int loop_calls;
static int display;

void firmware_setup(void) {
    setup_calls++;
}

void firmware_loop(void) {
    loop_calls++;
}

void *firmware_get_display(void) {
    return &display;
}

int test_setup_calls(void) {
    return setup_calls;
}

int test_loop_calls(void) {
    return loop_calls;
}
