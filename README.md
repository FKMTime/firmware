# FKM Timer

## Main Pinout

| #   | IO#   | ADC/STRAP/USB | USAGE          | Notes                  |
|-----|-------|---------------|----------------|------------------------|
| 0   | IO4   | ADC           | SPI SCK        |                        |
| 1   | IO5   | ADC           | SPI MISO       |                        |
| 2   | IO6   |               | SPI MOSI       |                        |
| 3   | IO7   |               |                |                        |
| 4   | IO8   | STRAP         |                |                        |
| 5   | IO9   | STRAP / IPU   |                | INTERNAL PULLUP (10kOhm) |
| 6   | IO10  |               | SHIFTER DATA   |                        |
| 7   | RXD/IO20|             | JACK INPUT     |                        |
| 8   | TXD/IO21|             | SHIFTER CLK    |                        |
| 9   | IO18  | USB           | USB D-         |                        |
| 10  | IO19  | USB           | USB D+         |                        |
| 11  | IO3   | ADC           | BUTTON INPUT   |                        |
| 12  | IO2   | ADC / STRAP   | BATTERY INPUT  |                        |
| 13  | IO1   | ADC           | SHIFTER LATCH  |                        |
| 14  | IO0   | ADC           |                |                        |

**Notes:**
* **STRAP:** Strapping pin
* **IPU:** Internal Pull-up (10kOhm).
* **USB:** Pins used for USB communication.

## Shifters Configuration

| SHIFTER # | USAGE          | BIT1    | BIT2    | BIT3    | BIT4    | BIT5    | BIT6    | BIT7    | BIT8    |
|-----------|----------------|---------|---------|---------|---------|---------|---------|---------|---------|
| 0         | BUTTONS MATRIX | BTN1    | BTN2    | BTN3    | BTN4    |         |         |         |         |
| 1         | LCD 16x2       | SPI_CS  | BL      | RS      | E       | D7      | D6      | D5      | D4      |
| 2         | DISPLAY DIGIT1 | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    |
| 3         | DISPLAY DIGIT2 | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    |
| 4         | DISPLAY DIGIT3 | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    |
| 5         | DISPLAY DIGIT4 | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    |
| 6         | DISPLAY DIGIT5 | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    |
| 7         | DISPLAY DIGIT6 | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    | TODO    |

## PCB
Pcb files are available as easyeda pro project.
File `FKM3.epro`

## 3D design 
You can view it and/or export it on onshape: 
 - [v1](https://cad.onshape.com/documents/a197fb44982d02cf4b905a34/w/d4d582c400841c8d2afd5356/e/063a9ed468737fb6f67b27f0?renderMode=0&uiState=65d2904a62856512eae60b89)
 - [v2](https://cad.onshape.com/documents/b9e6ecc4d8625f5993c3f9f8/w/500dd0485ca5c8c194e3d3fd/e/a40639e650796ab94408e0d7?renderMode=0&uiState=66239c4fa9b8ad7b949dae01)
