# Hardware Notes: STM32H723 USB Host and Display

> English version. [中文版](HARDWARE-NOTES.zh-CN.md)
>
> Records the peripheral wiring constraints and pitfalls this project runs into, so they do not
> have to be looked up twice.
> All conclusions are based on official manuals and community practice retrieved on 2026-11-19.
> Applies to: **LXB723ZG-P1** (STM32H723ZGT6, LQFP144, 1MB Flash / 564KB RAM) with an ST7789 SPI display.

---

## 0. Board identity, confirmed by measurement

Schematic: `LXB723ZG-P1原理图.pdf` from the board's documentation package (local paths are not
committed). Silkscreen mapping: `排针和引脚丝印对应关系图.png` from the same package.

Values read straight out of the chip take priority over any documentation:

| Item | Measured |
|---|---|
| CPUID (`0xE000ED00`) | `0x411FC272` → Cortex-M7 **r1p2** |
| Flash size (`0x1FF1E880`) | `0x0400` = **1024 KB** |
| DBGMCU_IDCODE (`0x5C001000`) | `0x10016483` → DEV_ID `0x483` (H72x/73x), REV `0x1001` |
| DBGMCU address | **`0x5C001000`**, **not** the F4/F7 `0xE0042000` (which reads back as all zeros) |
| Pre-existing firmware on the board | Yes. VTOR=`0x08000000`, AIRCR.PRIGROUP=3 (neither is the reset default) |

---

## 0.1 On-board peripheral assignment (confirmed from the schematic)

| Function | Pin / net | Note |
|---|---|---|
| **SPI display** (FPC10-0.5 connector U7) | SCK=`PG13`, MOSI=`PG14`, CS=`PG8`, DC=`PG15`, BL=`PG12` | On-board FPC socket; **but the same pins are also broken out to the 2×30 headers, so a 8-pin module can be wired with jumper wires** |
| User LED (blue) | **`PG7`** (1K series) | Can serve as a status indicator with no other peripheral involved |
| Power LED (red) | — | |
| **SWD + USART1 header** (2×4P SMD, H1) | SWCLK=`PA14`, SWDIO=`PA13`, USART1_RX=`PA10`, USART1_TX=`PA9`, RST, 3.3V | Debug port and serial share one header |
| **USB Type-C 16P** (U11) | wired directly to `PA11`(USB0_N) / `PA12`(USB0_P); **CC1/CC2 each pulled down 5.1kΩ** (R12/R13) | **Device only (DFU/CDC), cannot act as Host** |
| Expansion flash | W25Q64, OSPI, NCS=`PG6` | |
| TF card | SDMMC1 (`PC8`~`PC12`, `PD2`) | |
| DCMI camera | `PG9`/`PG10`/`PG11`, `PC6`/`PC7`, `PD3` | |
| Crystals | **HSE 25MHz** (PH0/PH1) + LSE 32.768kHz | Basis for later PLL configuration up to 550MHz |
| Power | +5V in → **RY3420** (U13, buck: IN/EN/FB/SW) → 3.3V | Two +5V pins broken out on the headers |

### Three corrections to earlier project assumptions

1. **How to wire the ST7789 (this section was wrong before and is now corrected)**
   The board does have an FPC10-0.5 display socket (U7), but it is **mechanically incompatible
   with the common 8-pin ST7789 module**, and no FPC ribbon cable ships with the board, so that
   socket is effectively unusable.
   Fortunately the pin list in the `IO口引出` (pin break-out) section includes
   `PG12/PG13/PG14/PG15/PG8`:
   ```
   PG14 PG15 PG12 PG10 PG13 PG11 PD7 PD5 PG9
   PG8  PG5  PG3  PD14 PD15 PG2  PG4 PG7
   ```
   ⇒ **Wire the 8-pin module straight to the headers with jumper wires: zero cost, no FPC
   adapter needed.** The FPC socket is in parallel with the headers on the same nets, so
   leaving it empty has no effect.

   8-pin module wiring:

   | Module pin | Connect to | Note |
   |---|---|---|
   | VCC | 3.3V | |
   | GND | GND | |
   | SCL / SCK | **PG13** | SPI6_SCK |
   | SDA / MOSI | **PG14** | SPI6_MOSI |
   | DC | **PG15** | LCD_DC |
   | CS | **PG8** | SPI6_NSS |
   | RES / RST | 3.3V or any GPIO | No dedicated LCD_RST net on the board |
   | BLK / BL | 3.3V (always on) or **PG12** | LCD_BL_PG12 exists on the board, possibly through a transistor |

   > Follow the silkscreen on your own module — the pin order differs between vendors, do not
   > copy the table order blindly.

2. **PA9/PA10 really are USART1** (same 2×4 header as SWD), so they are indeed occupied. This
   does not affect USB Host, because VBUS detection was optional anyway.
3. **The Type-C socket cannot act as USB Host** — CC1 and CC2 each have a 5.1kΩ pull-down, the
   standard UFP (device) configuration, and there is no VBUS supply path. To act as Host you must
   take `PA11`/`PA12` from the **pin break-out headers** and supply 5V yourself.

### A new opportunity: USB CDC (deferred)

Because Type-C goes straight to PA11/PA12, the STM32 can implement **USB CDC** itself — one
Type-C cable for both power and communication, no USB-TTL needed. But under Rust `no_std` that
means writing a USB device stack (48MHz clock, endpoint configuration, descriptors), which is a
lot of work and **not suitable for P0/P1**. Listed as a product-form optimisation after P2.

---

## 1. Background: Nucleo versus a core board

NUCLEO-H723ZG can act as a USB Host directly because it carries a kit of three parts; **a core
board usually only breaks out the MCU pins, so all three have to be added yourself.**

| Part | Purpose | Nucleo (MB1364) | Core board |
|---|---|---|---|
| Micro-AB socket | USB physical interface | CN13 | ❌ bring your own |
| **VBUS power switch** | 5V supply + current limit + overcurrent flag | driven by PD10, PG7 reports overcurrent | ❌ **must be added** |
| USBLC6-2 ESD protection | D+/D- electrostatic protection | on board | ❌ recommended |
| USB role detection ID | PA10 | CN13 pin4 | can be skipped for a fixed Host |

**Nucleo CN13 pinout** (UM2407 Table 16):

| pin | Signal | MCU pin | Note |
|---|---|---|---|
| 1 | VBUS | PA9 | 5V supply, also used for VBUS detection |
| 2 | DM | PA11 | USB_OTG_HS_DM |
| 3 | DP | PA12 | USB_OTG_HS_DP |
| 4 | ID | PA10 | role detection (can be skipped for a fixed Host) |
| 5 | GND | — | |

USB-related solder bridges on the Nucleo (UM2407 Table 12):
SB21 (PA11), SB22 (PA12), SB23 (PA9), SB24 (PA10), SB77 (PD10 power enable), SB76 (PG7 overcurrent).

---

## 2. The H723 USB peripheral

### 2.1 Capabilities

- **USB 2.0 high-speed/full-speed device/host/OTG**, with dedicated DMA
- **On-chip FS PHY** (12 Mbps is enough — a USB mouse is FS or LS)
- ULPI interface for an external HS PHY (not needed here)
- Relevant pins: PA11 = `OTG_HS_DM`, PA12 = `OTG_HS_DP`, PA9 = `OTG_HS_VBUS`, PA10 = `OTG_HS_ID`

**A mouse is a USB full-speed/low-speed device, so the on-chip FS PHY is sufficient — no ULPI
chip and no 48MHz crystal needed.**

### 2.2 Clock requirement (easy to get wrong)

USB FS requires a reference clock error **< 500ppm**.

The H723 clock tree offers three sources for the USB Clock Mux: **PLL1Q / PLL3Q / HSI48**.

> **HSI48 is not accurate enough** (internal RC source) — the PLL is mandatory.
> Getting this wrong shows up as "enumeration fails" or "sometimes it is detected, sometimes not",
> which is very hard to debug. CubeMX checks it, but confirm the generated PLL divider is right.

### 2.3 VBUS supply (ST's own wording)

> "On-chip 5 V VBUS generation is not supported. For this reason, a charge pump or,
> **if 5 V are available on the application board, a basic power switch must be added
> externally to drive the 5 V VBUS line.** The external charge pump can be driven by any
> GPIO output. **This is required for host-only configuration.**"

The host side must additionally:
- provide its own 5V supply for the downstream device
- **include current limiting** (so a shorted device cannot drag down the internal rails)
- **detect overcurrent** and report it to the controller

### 2.4 VBUS detection (AN4879)

PA9 is 5V tolerant, but its **absolute maximum is VDD + 4V**, and **the MCU must not be
unpowered while 5V is already present on PA9**.

Standard divider (for VDD between 3.0 and 3.6V):

```
VBUS ──[ 33kΩ ]──┬──→ PA9
                 │
              [ 82kΩ ]
                 │
                GND
```

That gives about 1.7V.

> **Once you use a divider you must disable the peripheral's VBUS sensing function**
> (`OTG_GCCFG.VBDEN = 0`) and read PA9 as a plain GPIO input. AN4879 says:
> "This method uses the regular GPIO functionality, the VBUS sensing function
> (OTG_GCCFG.VBDEN), embedded in PA9, should not be enabled."

**A fixed Host configuration can skip detection entirely**: pull EN high at power-up and VBUS is
always on. An ST community developer confirms: *"Even without [Drive_VBUS_HS], I can still detect
my USB."*

---

## 3. Wiring scheme

### 3.0 Essential addition: Host mode needs external 15kΩ pull-downs (easy to miss)

**Missing from the previous revision.** In the USB specification:

- **Device side**: pulls 1.5kΩ up on D+ (full speed) or D- (low speed); the device adds it
- **Host side**: **must pull D+ and D- down to ground with 15kΩ each**

What the host-side 15kΩ pull-downs do:
1. bias the bus to the J state (idle) when no device is attached
2. let the host detect the device-side 1.5kΩ pull-up to identify speed (FS pulls D+, LS pulls D-)
3. prevent the bus from floating

SparkFun's USB Host tutorial says:
> "Additionally, a **15kΩ pull-down resistor is needed on each D+ and D- line**.
> Attach one resistor from D+ to GND, and attach another from D- to GND."

components101's USB-A female page requires the same 15kΩ pull-downs.

> The STM32 USB peripheral has an **integrated 1.5kΩ pull-up** (that is for Device mode),
> but **the 15kΩ pull-downs that Host mode needs must be added externally**.
> Symptom when omitted: power is detected but **enumeration fails / the device is not recognised**.

### 3.1 Complete wiring table

| USB-A female | → | STM32H723 | Note |
|---|---|---|---|
| pin3 D+ | → | **PA12** | `OTG_HS_DP` |
| pin2 D− | → | **PA11** | `OTG_HS_DM` |
| pin1 VBUS | ← | power switch OUT | also pin4 on the receptacle |
| pin4 VBUS | ← | same as pin1 | usually shorted inside a USB-A female |
| shell | → | GND | the metal shell must be grounded |
| GND | ↔ | core board GND | common ground is mandatory |

**Add a 15kΩ pull-down from each of D+ and D− to GND** (see 3.0).

VBUS supply and detection branch:

```
core board 5V ──► TPS2051D IN
                 ├─ EN  ◄── any GPIO (e.g. PB0)
                 ├─ OC  ──► another GPIO (optional, overcurrent flag)
                 └─ OUT ──► receptacle pin1 / pin4
                          └─► [33kΩ] ──► PA9 ──► [82kΩ] ──► GND
```

### 3.2 USB-A female pinout (confirmed on the physical part)

**Confirmed on hardware (2026-11-19, "立式USB母座带PCB板 USB2.0母座已焊接好", 1.15 CNY)**:
the silkscreen reads **top to bottom**:

```
┌─────────────┐
│  GND        │  → core board GND
│  D−         │  → PA11   (+ 15kΩ pull-down to GND)
│  D+         │  → PA12   (+ 15kΩ pull-down to GND)
│  VBUS       │  ← TPS2051D OUT ← core board 5V
└─────────────┘
```

The metal shell of the receptacle also goes to GND.

> This particular part has clear, unambiguous silkscreen, so no further measurement is needed.
> If you switch to a different breakout, still check: the USB-A standard is
> pin1=VBUS / pin2=D− / pin3=D+ / pin4=GND or VBUS (varies by vendor).

**Note when ordering**: if this exact part is unavailable, pick a "USB-A female with PCB" and
ask the seller "what is the order of the four pins from top to bottom?".

### 3.3 Current-limited switch selection

| Part | Package | Current | Note |
|---|---|---|---|
| **TPS2051B/D** | SOT-23-5 / SOIC-8 | 0.5A continuous, 0.7–1.2A limit | Most common; OC is an active-low open-drain output |
| TPS22919-style load switch | SOT-23-6 | limit built in | Smaller, no separate OC |
| P-MOS + current sense resistor | discrete | design your own | Cheapest, but overcurrent protection is crude |

Typical TPS2051 wiring: `VIN` ← 5V, `OUT` → VBUS, `EN` ← GPIO (high = on),
`OC` → GPIO (active-low open drain, needs a pull-up), 0.1µF input / 10µF output decoupling.

**Watch out for reverse current**: TPS2051B allows OUT→IN current flow but **does not limit it**.
If the receptacle side might be externally powered, add a parallel diode or use a bidirectional part.

### 3.4 Bill of materials (minimum viable USB Host, verified)

| Item | Qty | Approx. price | Note |
|---|---|---|---|
| USB-A female with PCB (vertical, USB2.0) | 1 | 1.15 CNY | **Confirmed model**, silkscreen GND/D−/D+/VBUS top to bottom |
| TPS2051D (SOT-23-5) | 1 | ~1 CNY | VBUS limiting + overcurrent |
| **15kΩ resistor** | **2** | ~0.1 CNY | **D+ / D- pull-downs, mandatory** |
| 33kΩ / 82kΩ resistor | 1 each | ~0.1 CNY | VBUS detection (optional for a fixed Host) |
| 0.1µF + 10µF capacitor | 1 each | ~0.2 CNY | switch decoupling |
| USB-A male-to-male cable | 1 | have it | to the mouse |

**Total about 3 CNY.**

> **To be confirmed**: does the core board break out **5V**? The receptacle VBUS needs a 5V source.
> If 5V only feeds the LCD backlight and is not on a header, fly a wire from the 5V test point,
> or add a 3.3V→5V boost module (about 1 CNY).

### 3.5 Capacitor selection (specific to this project)

**Conclusion: use MLCC (monolithic/surface-mount ceramic), dielectric X7R or X5R, voltage rating
≥ 10V.**

| Type | Characteristics | Performance as VBUS decoupling | Verdict |
|---|---|---|---|
| **Monolithic/SMD MLCC** | ceramic dielectric, extremely low ESR, near-zero leakage | **Best**, absorbs USB hot-plug transients | ✅ **Use this** |
| Ceramic disc capacitor (leaded) | also ceramic, but bulky, high ESL, hard to get large values | usable, ugly parasitics | ⚠️ Fallback |
| Aluminium electrolytic | large capacity, **high ESR**, polarised, leaky, finite life | **Unsuitable for high-frequency decoupling**, only for bulk energy | ⚠️ Large capacities only |

**The key difference**: high ESR in aluminium electrolytics ⇒ slow response to high-frequency
transients (USB hot-plug inrush). USB decoupling must rely on low-ESR ceramic.

#### 3.5.1 MLCC DC-bias derating (the easiest pitfall to hit)

X7R/X5R MLCCs lose a lot of real capacitance under DC bias (the dielectric constant falls with
electric field — inherent physics, not a quality problem).

Typical measured figures, 0805 package:

| Nominal | Rating | Actual capacitance at 5V |
|---|---|---|
| 10µF | 6.3V | **only 2–4µF left** (more than 60% gone) |
| 10µF | 16V | about 6–8µF |
| 10µF | 25V | about 8–9µF |

**Rule: a capacitor connected to 5V must be rated ≥ 10V.**
**Never use Y5V / Z5U** (they derange even harder — a 6.3V part can be down to 20%).

#### 3.5.2 Capacitor purchase table for this project

| Purpose | Value | Rating | Dielectric | Package | Qty |
|---|---|---|---|---|---|
| Switch input decoupling | 0.1µF (100nF) | ≥16V | X7R | 0603/0805 | 2 |
| Switch output bulk | 10µF | **≥16V** | X7R or X5R | 0805/1206 | 2 |
| (optional) VBUS stabilisation | 100µF | 10V | **aluminium electrolytic** | Φ6×11 | 1 |

- 0.1µF in 0603 is enough — the USB traces already carry stray capacitance, and **placement
  close to the chip pins matters more than the value**
- 10µF in 0805 with **16V** (not 6.3V)
- The 100µF electrolytic is optional: it makes VBUS "harder" during a brief droop or inrush,
  preventing the mouse from resetting. The USB specification caps VBUS capacitance at
  **120µF** (to avoid burning the host with inrush current); 100µF is within that.
  **It works without it, but it is steadier with it.**

#### 3.5.3 Soldering notes

- TPS2051D is SOT-23-5; keep the iron at ≤320°C, a few seconds per part
- The electrolytic is **polarised** — the long leg is positive; reversed it bulges and can burst
- Solder the two 15kΩ pull-downs straight onto the back of the receptacle board (D+/D− to GND)
  and skip the flying wires

**Or** buy a "USB OTG Host adapter board (with TPS2051 + USB-A female)", 8–15 CNY, and solder
only four wires.

### 3.6 The reality of the software stack

| Route | Effort | Risk |
|---|---|---|
| Write the whole USB Host stack in Rust | large | enumeration, endpoint scheduling and descriptor parsing are full of traps |
| ST's C library + Rust FFI | medium | introduces a C dependency, which breaks the "pure Rust + verifiable" story |
| **Use serial first, revisit later** | **zero** | none |

ST's USB Host class drivers (UM1720 `USB_Host_Library/Class`) are all C:
`usbh_hid.c` / `usbh_hid_parser.c` / `usbh_hid_mouse.c` / `usbh_hid_keybd.c` /
`usbh_msc.c` / `usbh_cdc.c`.

---

## 4. Three pitfalls

### Pitfall 1: the diode inside a cut-up Type-C adapter

Internal routing of a Type-C to USB-A OTG adapter (the kind you plug a mouse into a phone with):

```
USB-A pin3 (D+) ──► Type-C TX1 (A6/B6)
USB-A pin2 (D−) ──► Type-C RX1 (A7/B7)
USB-A pin1/4 (VBUS) ──► [possibly a Schottky diode] ──► Type-C VBUS (A4/A9/B4/B9)
```

**The Type-C side also carries CC1/CC2 pull-downs (Rd 5.1kΩ)** — that is how a phone tells
"I am a device, please supply me with power".

Cutting off the Type-C end gives you a usable USB-A pigtail, but:

> **Measure the continuity of the VBUS path with a multimeter.**
> If the diode is there, current is blocked in that direction and external 5V cannot reach the
> receptacle VBUS — the symptom is "the mouse does not light up". Short the diode out.

**The cleaner approach: buy a 4-pin USB-A female directly**, which sidesteps the diode, the CC
resistors and the TX/RX mapping question entirely.

**Note**: a Type-C to USB-A adapter **has no USB Host supply capability** — it is a data
adapter, VBUS must be provided externally.

### Pitfall 2: PA11/PA12 may already be taken by CAN

Pin assignment of a well-known STM32H743 board (from an open-source project's BRINGUP.md):

```
CAN1   PA11 RX / PA12 TX, 500 kbit/s
debug serial USART1  PA9 TX / PA10 RX, 115200
```

**If PA11/PA12 are taken by CAN, USB cannot be used at all.**
Always check the schematic of the board actually in your hands before routing wires.

Likewise **PA9 is commonly USART1_TX** (the debug console), so if PA9 is needed for VBUS
detection, the debug serial has to move to other pins (e.g. PA2/PA3).

### Pitfall 3: 48MHz clock accuracy

See 2.2. **The PLL is mandatory — HSI48 is not accurate enough.**

---

## 5. ST's official dual-role example (NUCLEO-H723ZG)

Key points of the ST community article "How to configure STM32 as USB dual role":

- The validation platform is exactly **NUCLEO-H723ZG**
- CubeMX **does not natively support dual role** — you need two projects (Host and Device) and
  copy files between them by hand
- Host project configuration: `USB_OTG_HS` → Host only (internal FS PHY) → Activate VBUS
- In **Platform Settings**, `Drive_VBUS_HS` must be given a GPIO (otherwise a warning pops up
  during generation and `usbh_platform.c` is not produced)
- The article connects with a USB 2.0 A-type female to micro-B male cable

The matching USB Host class drivers shipped with STM32Cube USB:

| Class | File |
|---|---|
| HID mouse and keyboard | `usbh_hid.c` / `usbh_hid_parser.c` / `usbh_hid_mouse.c` / `usbh_hid_keybd.c` |
| Mass storage | `usbh_msc.c` |
| CDC virtual serial port | `usbh_cdc.c` |

**Reference point**: because this example was completed on a NUCLEO, the feasibility of the
protocol stack is officially validated. The incremental work for a core board is the **power
circuit and the connector** — the protocol stack effort is identical to the Nucleo's.

---

## 6. Display (ST7789 and the N647)

### 6.1 The ST7789 has no CLUT

COLMOD (0x3A) supports only **12/16/18 bits** (RGB444 / RGB565 / RGB666),
with **no indexed mode and no CLUT register**.

⇒ An L8 framebuffer cannot drive it directly; it must be expanded to RGB565 first:
- DMA2D supports indexed → RGB565 pixel format conversion (in hardware)
- **SPI bandwidth is the limit**: 240×320×2 = 153KB per frame, SPI 40MHz ≈ 5MB/s
  ⇒ **a full-screen refresh takes about 30ms**
- The ST7789's GRAM plus the CASET(0x2A)/RASET(0x2B) window mechanism allow refreshing only
  dirty rectangles, which mitigates this

Other points:
- The window address must be set with CASET/RASET, otherwise data can spill past the visible
  area (shows up as scrolling stripes)
- On some modules the physical origin is not GRAM (0,0) (a 1.14" panel may start at column 40).
  Prefer adding an offset in CASET/RASET over changing the porch registers
- COLMOD = 0x05 (16-bit) with the low two bits of RAMCTRL = 0 means RGB565 **big endian**
  (high byte first)
- Red/blue swapped → check the RGB bit in MADCTL; green blocks with horizontal stripes → check
  the byte order

### 6.2 LTDC natively supports L8 + a per-layer CLUT

STM32 LTDC manual (applies to H7 / N6):

> "**Color look-up table (CLUT) up to 256 colors (256x24-bit) per layer**"
>
> Input pixel formats include "**L8 (8-bit luminance or CLUT)**", AL44, AL88

- The CLUT is written per layer through `LTDC_LxCLUTWR`, **each layer independent**
- The LTDC reads indices straight out of the L8 framebuffer and expands them with the hardware
  CLUT: **zero CPU involvement, no DMA2D participation needed**
- The AL44 format only needs 16 colours loaded, with the index filled by repeating the 4-bit L
  channel into 8 bits (L0→0x00, L1→0x11, L2→0x22, … L15→0xFF)

### 6.3 Comparison of the two display paths

| | H723 + ST7789 | N647 + 4.3" RGB LCD |
|---|---|---|
| Bus | SPI (CPU/DMA pushes pixels) | parallel RGB (LTDC scans out) |
| L8 + CLUT | ✗ needs software expansion | ✓ **native in hardware** |
| CPU cost of a screen refresh | high | **zero** |
| Touch | ✗ | ✓ GT911 (I2C + external interrupt, 5 points, no calibration needed) |
| Effect on real-time behaviour | **yes** (bus contention) | **almost none** |
| Memory | 150KB double buffered / 564KB RAM | **768KB** double buffered / 4.2MB + 32MB HyperRAM |

### 6.4 Extra graphics hardware on the N6

- **Chrom-GRC (GFXMMU)**: programmable display shape, physically stores only the visible pixels,
  4 virtual buffers
- **Neo-Chrom GPU2D**: supports L8, CLUT (256 entries), ARGB8888, RGB565/888 and more;
  command-list driven DMA, so CPU overhead is very low
- ST's own words: 4.2MB of RAM "could store a double frame buffer for a 1280 x 800 display"

### 6.5 Display memory and placement (applies across platforms)

- **The framebuffer must be marked Non-Cacheable** (MPU configuration).
  ST note DM00272913 lists four approaches; a community measurement with TouchGFX mixing
  AXI (write-through) and SDRAM (write-back) still produced **ghosting and HardFault** even
  with cache maintenance restricted to the framebuffer.
  ⇒ Choose Non-Cacheable.
- **The framebuffer must not live in DTCM**: DMA1/DMA2 sit in the D2 domain and cannot reach
  ITCM/DTCM (0x20000000); only MDMA (D1 domain) can.
- **LTDC layer framebuffer start address must be 16-byte aligned**, 32 bytes is better
  (the cache line is 32 bytes, which avoids the granularity trap in cache maintenance).

---

## 7. Open items and decision log

| Item | Status |
|---|---|
| USB Host hardware | **Deferred**. The P0–P2 phases use the serial CLI and Ethernet web; no mouse needed |
| USB Host input abstraction | **Fix it now**: a unified `InputEvent` stream, so adding a mouse or touch is just another Source with zero UI changes |
| H723 core board pin usage | **To check**: is PA11/PA12 taken by CAN1, is PA9 taken by the debug serial |
| Ethernet | If the core board carries a PHY + RJ45 (some STM32H743 boards use YT8512C / DP83848), a cable is all it takes — no extra hardware |

---

## 8. Sources

| Conclusion | Source |
|---|---|
| H723 USB capabilities and pins | DS13313 datasheet section 3.x, AN4879 |
| Nucleo CN13 pinout and USB solder bridges | UM2407《STM32H7 Nucleo-144 boards (MB1364)》Table 12 / Table 16 |
| On-board VBUS switch driven by PD10, PG7 reports overcurrent | MB1364 schematic (deepbluembedded teardown) |
| VBUS requires an external power switch | ST community《Warning during STM3CubeMX setting in USB_HOST》 |
| Divider detection and disabling VBDEN | AN4879《Introduction to USB hardware and PCB guidelines for STM32 MCUs》section 2.6 |
| VBUS current-limited switch requirements | TI TPS2051B-Q1 datasheet section 9.2.2 |
| Official dual-role example | ST community《How to configure STM32 as USB dual role》 |
| USB Host HID class driver list | UM1720《STM32Cube USB》 |
| ST7789 colour format and window mechanism | ST7789V manual, several driver implementations |
| **Host mode needs 15kΩ pull-downs** | SparkFun《USB Type A Female Breakout Hookup Guide》, components101 USB-A female page |
| USB-A female pinout | components101《USB Type-A (Female) Connector》, ARIAT Tech breakout description |
| LTDC L8 and CLUT | STM32 LTDC manual, wiki.st.com LTDC feature page |
| Four approaches for D-Cache coherency | DM00272913《Level 1 cache on STM32F7/H7 series》 |
| DMA cannot reach DTCM, ghosting case | ST community《DMA is not working on STM32H7 devices》《Granular D-Cache maintenance operations...》 |
| N647 on-board 4.3" RGB LCD + GT911 | board vendor's official wiki, ATK-MD0430R module repository |
| N6 Chrom-GRC / Neo-Chrom | STM32N647xx datasheet sections 3.12 / 3.30 |
| PA11/PA12 taken by CAN | cgzjs/stm32h743-industrial-gateway BRINGUP.md |

## 9. Switching to 550 MHz breaks the SWD link

**Attempted 2026-11-22. The PLL parameters are verified correct; the switch itself is unreachable.**

### What is verified

The PLL configuration copied from the vendor reference example is right. Readback after lock:

```
# clk[3] HSE ready, RCC_CR=0x00034025
# clk[5] PLL locked: CR=0x03034025 DIVR=0x010100db
```

`DIVR=0x010100DB` decodes to `N1=219`, `P1=0`, `Q1=1`, `R1=1` — exactly "N/P/Q/R stored as value
minus one". `CR` has `HSEON|HSERDY|PLL1ON|PLL1RDY` all set.

So HSE 25 MHz / M=10 / N=220 / P=1 = 550 MHz is a valid configuration and the chip locks it.

### Where it dies — localised without a debug link

The debug link dies during this sequence, so "print and look" stops working. Instead the firmware
writes a **progress marker into `.uninit`** at every step. `.uninit` is uninitialised RAM that
cortex-m-rt does not clear, and it survives a flash erase, so the markers can be read back with no
live connection at all:

```bash
probe-rs erase --chip STM32H723ZGTx --connect-under-reset   # recover the AP
probe-rs read  --chip STM32H723ZGTx b32 0x24003488 8        # read the markers
```

```
24003488: 00000000 c0de0006 c0de0007 00000000 ...
```

### The VOS encoding bug (found and fixed)

For a long time the failures were blamed on "VOS0 not taking effect". That was half right.

`stm32h7xx_hal_pwr_ex.h` defines `PWR_REGULATOR_VOLTAGE_SCALE0` **twice**, under different
conditions, because some H7 parts put voltage scaling in `SRDCR` and others in `D3CR`:

```c
// SRDCR parts        SCALE0 = 0b11   SCALE1 = 0b10   SCALE2 = 0b01   SCALE3 = 0b00
// D3CR parts (H723)  SCALE0 = 0b00   SCALE1 = 0b11   SCALE2 = 0b10   SCALE3 = 0b01
```

The code was written against the first table, so writing `0b11` produced **SCALE1**, not the
highest level. Reset is `0b01` = **SCALE3**, whose ceiling is around 200 MHz — which is exactly
where the working configuration sat.

Verified from the host, with no firmware involved:

| Write to `PWR_D3CR` | `PWR_D3CR` reads | `PWR_CSR1` (ACTVOS) |
|---|---|---|
| reset | `0x6000` → SCALE3 | `0x6000` |
| `0xE000` | `0xE000` → SCALE1 | `0xE000` |
| **`0x0000`** | **`0x2000` → SCALE0** | **`0x2000`** |

`PWR_CSR1.ACTVOS[15:14]` is the field that matters — it reports the level actually in force.
`PWR_D3CR.VOSRDY` only says the write was accepted. And `ACTVOSRDY` is `PWR_CSR1` bit 13, **not**
`PWR_CR3` bit 13 (H723 has no such bit in CR3).

### The switch works when the core is not executing

The most informative result came from taking the CPU out of the loop entirely: halt the core,
then perform the **same** register sequence from the host with `probe-rs write`.

```
[7] 切换 SYSCLK -> PLL1  (关键一步)
  OK   SWS = PLL1  (0x0000001b)，第 0 次读
```

`CFGR = 0x0000001b` means `SW = 3` **and** `SWS = 3` — the RCC completed the switch to 550 MHz.

So everything about the configuration is correct: the PLL parameters, VOS0, the flash wait states,
the bus dividers, and 550 MHz as a target. **The problem is the CPU executing across the
transition, not the clock configuration.**

The reverse control confirms the RCC is fine: `SW=2` switches to HSE successfully, and `SW=0`
returns to HSI — only `SW=3` fails when the firmware does it.

### One trap in this experiment (and a retraction)

The first version of the host-side script wrote `PLLCFGR = 0x04` outright and got "the switch
fails". That was **an artifact of the script**: `PLLCFGR` resets to `0x01FF0000`, where bits 16–24
are `DIVP1EN` / `DIVQ1EN` / `DIVR1EN` — the output-divider enables. Overwriting the register
disabled the P output, so of course the PLL could not drive SYSCLK.

The firmware always used read-modify-write, so it never had this bug. Had this not been caught, the
conclusion would have been exactly wrong.

### Retraction: the AXI test did not refute the fetch hypothesis

Earlier, 200 MHz CPU with AXI at 200 MHz was measured working, and that was taken as proof that
"instruction fetch is not the problem". **That conclusion was too strong.** It only showed that at
CPU = 200 MHz there is enough margin in the fetch path. At CPU = 400/550 MHz the margin is gone,
and the failure appears.

Combined with the host-poke result, the best-supported explanation is now:

> SYSCLK jumps from 64 MHz to the target in a single write. The instructions immediately after that
> write are fetched from flash, and at 400+ MHz that fetch fails. The HardFault handler is also in
> flash, so nothing runs and nothing is logged.

### The RAM-execution test: inconclusive, and a mistake on my part

The switch was re-implemented as a self-contained assembly routine (no `bl`, no absolute jumps),
copied byte-for-byte into a RAM buffer at startup, and called through a function pointer — so the
switch would execute entirely from RAM and never touch flash.

The copy was verified to have happened:

| Marker | Value | Meaning |
|---|---|---|
| 4 | `0xC0DE1022` | 0x22 = 34 bytes copied |
| 6 | `0x08000476` | `__ram_switch_start` |
| 7 | `0x08000498` | `__ram_switch_end`, exactly 0x22 bytes later |
| 5 | **not** `0xC0DE200x` | the call never returned |

So the routine was emitted, copied and entered — and the CPU still died inside it.

**That does not support the fetch hypothesis**, and it does not cleanly refute it either: those 34
bytes are hand-written assembly, and one wrong encoding would fault in exactly the same way. The
experiment cannot tell "fetch from flash is the problem" apart from "my assembly is wrong".

### A confounder found the hard way: `rprintln!` can block

While chasing the RAM-execution path, one observation changed the diagnosis:

```
mark(2, 0xC0DE_0007)          // 总线分频已设置
rprintln!("# clk[7] dividers: …");   // ← 这一行从未出现
```

The marker was written, the very next statement never produced output. The firmware had not died
in the clock switch — **it was blocked inside the print**.

RTT is initialised with `ChannelMode::BlockIfFull` (deliberately: `NoBlockSkip` silently dropped
30 % of samples in experiment 01). Once the D3 prescaler changes, the debug access port can lose
sync and probe-rs stops draining the buffer. The buffer fills, and the next `rprintln!` waits
**forever**.

That is indistinguishable from a hang in the clock switch, and it means several earlier "the CPU
died at the switch" observations are **not trustworthy**. The diagnostic prints themselves may have
been the thing that hung.

Consequence adopted: **no printing at all during clock configuration.** The `CLK_VERBOSE` constant
defaults to `false`, and progress is recorded only in `.uninit` markers, which never block.

### Calling copied code: the Thumb bit

Trying to execute the switch from a RAM copy faulted immediately, and the reason was not the clock:

```rust
// 错：缓冲区 4 字节对齐，bit0 = 0
let f: fn(..) = transmute(buf_addr);
```

On Cortex-M a function pointer carries the Thumb state in **bit 0**, which must be 1. Calling an
even address makes the core try to execute Thumb code in ARM state and fault at once — presenting
exactly like "the switch killed the CPU".

```rust
let f: fn(..) = transmute((buf_addr | 1) as *const u8);
```

The RAM path still does not work (the call faults even after this fix, pointing at a second defect
in the hand-written routine), so the experiment was reverted rather than left in. But two things
came out of it that are worth keeping: the `BlockIfFull` confounder above, and a self-test pattern
that answers "does this routine execute at all?" without any side effects — call it with
`SW = 0` / expected `SWS = 0`, which is a no-op on a chip already running from HSI.

### A mistake worth recording

While reading the progress markers back, a hard-coded address (`0x24003488`) was used even though
the firmware prints the real one on every boot. Adding the 64-byte RAM buffer to `.bss` moved
`.uninit` to `0x240034C8`, so several reads landed 64 bytes into the middle of the block and
returned stale values. Two intermediate conclusions drawn from those reads were wrong.

The firmware had been printing the right address the whole time (`# uninit block at 0x240034C8`).
**Use the printed value, never a remembered one.** This is the same class of error as the earlier
`PLLCFGR` overwrite, and it cost more time than either of the real bugs.

### Recovery

`probe-rs reset --connect-under-reset` does **not** recover a chip stuck after a failed clock
switch — most likely because nRESET is not wired on this board (it was listed as "recommended, not
required" in the wiring table above).

The only reliable recovery is to erase the flash while holding reset:

```bash
probe-rs erase --chip STM32H723ZGTx --connect-under-reset
```

Verified many times. Erasing does **not** clear RAM, which is what makes the progress-marker
technique work.

### What the workaround already achieves

The store-and-reset transport **is implemented and verified at 200 MHz** (see `exp/README.md`):
results go into `.uninit`, `SYSRESETREQ` is issued, and after reset the firmware prints the stored
data over RTT.

Two properties were confirmed the hard way:

- `.uninit` really does survive a reset **and** a reflash. A stale block from a previous run
  short-circuited a fresh measurement — the firmware happily printed yesterday's data as if it were
  new. Fixed by clearing the block before measuring.
- the block layout and `MAGIC` round-trip correctly.

What is missing is a clock configuration that survives long enough to store anything.
### Stopped here, on purpose

The investigation was halted at this point rather than pushed further. The reason is not the
difficulty of the remaining problem — it is that **the instrument kept producing contradictory
readings**, and continuing would have produced more noise than knowledge.

Concretely, with identical code, three consecutive runs reported:

| Run | Last progress marker | Reading |
|---|---|---|
| A | `0xC0DE_0007` (dividers set) | died in or just after the divider writes |
| B | `0xC0DE_0004` (HSE ready) | died before the PLL locked |
| C | `0xC0DE_0004` (HSE ready), and the failure-path diagnostic never ran | died inside code that had just succeeded |

The PLL itself locked in earlier runs with the same parameters (`CR=0x03034025`,
`DIVR=0x010100db`), so B and C are not reproducible findings — they are symptoms of an
environment that is no longer in a known state between runs.

### What is actually established

These hold up under repeated measurement:

1. **The RCC can switch to PLL1 at 550 MHz.** With the core halted, the same register sequence
   driven from the host reaches `SWS = PLL1` (`CFGR = 0x0000001b`). So the configuration is
   correct and the target frequency is reachable on this board.
2. **`rprintln!` can block forever and fake a hang.** RTT uses `BlockIfFull`; once the D3
   prescaler change kills the debug link, the next print waits permanently. This invalidates
   several earlier "the CPU died at the switch" observations.
3. **`PWR_D3CR` VOS encoding**: `SCALE0` is `0b00`, not `0b11`. The original code wrote `SCALE1`
   and reset is `SCALE3`, whose ceiling is around 200 MHz. Fixed and verified via `ACTVOS`.
4. **Calling copied code requires bit 0 set** (Thumb). Calling an even address faults exactly like
   a clock failure.

### What is not established

Whether the firmware can complete the switch while executing from flash at 400/550 MHz. Every
attempt to answer it was confounded — by print blocking, by a stale `.uninit` block from a previous
run short-circuiting the measurement, by an address that moved between builds, and finally by a
chip whose state between runs is not reproducible.

### The next step, when this is picked up again

Not more register poking. In order:

1. **Make runs independent.** Add an explicit "factory reset" at boot: clear the `.uninit` block
   and write a boot counter, so a run can prove it started fresh. Several of the confusing results
   above came from a stale block.
2. **Make the instrument trustworthy before using it.** The progress-marker technique is sound —
   it survives resets and flash erases and needs no debug link — but it needs a per-build nonce so
   stale data can be recognised instantly. One `mark(0, NONCE)` would have saved hours.
3. **Then, and only then**, re-ask the original question with zero printing.

Doing 1 and 2 first is not bureaucracy: every wrong turn in this session traces back to skipping
them.
