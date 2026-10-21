/* Hooks called by the vendor's KM4 main; preserve its startup and RTOS setup.
 * The application itself is the Rust staticlib linked into this image.
 */
void petlibro_app_start(void);

void pre_example_entry(void)
{
}

void example_entry(void)
{
    petlibro_app_start();
}
