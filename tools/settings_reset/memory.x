/* Adafruit nRF52 bootloader: application starts at 0x26000 */
MEMORY
{
    FLASH : ORIGIN = 0x00026000, LENGTH = 824K
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
