#include <Arduino.h>
#include <EEPROM.h>
#include <ArduinoJson.h>
#include <Update.h>
#include <Wire.h>
#include "soc/soc.h"
#include "soc/rtc_cntl_reg.h"

#include "ws_logger.h"
#include "pins.h"
#include "utils.hpp"
#include "version.h"
#include "globals.hpp"
#include "lcd.hpp"
#include "buttons.hpp"
#include "state.hpp"
#include "radio/radio.hpp"
#include <stackmat.h>

void core2(void *pvParameters);
inline void loop2();
void rfidLoop();
void sleepDetection();
void stackmatLoop();

void setup() {
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 0);

  pinMode(BUTTON1, INPUT_PULLUP);
  pinMode(BUTTON2, INPUT_PULLUP);
  pinMode(BUTTON3, INPUT_PULLUP);
  pinMode(BUTTON4, INPUT_PULLUP);
  pinMode(BAT_ADC, INPUT);
  pinMode(DIS_DS, OUTPUT);
  pinMode(DIS_STCP, OUTPUT);
  pinMode(DIS_SHCP, OUTPUT);

  Serial.begin(115200);
  Logger.begin(&Serial);
  EEPROM.begin(128);
  Wire.begin(LCD_SDA, LCD_SCL);
  readState();
  clearDisplay(0);
  lcdInit();

  delay(100);
  currentBatteryVoltage = readBatteryVoltage(BAT_ADC, 15);
  float initialBat = roundf(voltageToPercentage(currentBatteryVoltage));

  Logger.printf("ESP ID: %x\n", getEspId());
  Logger.printf("Current firmware version: %s\n", FIRMWARE_VERSION);
  Logger.printf("Build time: %s\n", BUILD_TIME);
  Logger.printf("Battery: %f%% (%fv)\n", initialBat, currentBatteryVoltage);

  lcdPrintf(0, true, ALIGN_LEFT, "ID: %x", getEspId());
  lcdPrintf(0, false, ALIGN_RIGHT, "%d%%", (int)initialBat);
  lcdPrintf(1, true, ALIGN_LEFT, "VER: %s", FIRMWARE_VERSION);

  Serial1.begin(STACKMAT_TIMER_BAUD_RATE, SERIAL_8N1, STACKMAT_JACK, 255, false);
  stackmat.begin(&Serial1);
  SPI.begin(RFID_SCK, RFID_MISO, RFID_MOSI);

  buttonsInit();
  mfrc522.PCD_Init();
  
  initWifi();
  lcdClear();
  clearDisplay();

  // IDK WHY BUT MY STACKMAT IMPLEMENTATION IS BUGGY WHEN THERE IS A LOT OF 
  // DATA INSIDE Serial BUFFER, SO IT WILL FIX IT
  while(Serial1.available()) { Serial1.read(); } 

  initState();
  xTaskCreatePinnedToCore(core2, "core2", 10000, NULL, 0, NULL, 0);
  WRITE_PERI_REG(RTC_CNTL_BROWN_OUT_REG, 1);
}

void loop() {
  if (update) {
    webSocket.loop();
    return;
  }

  stateLoop();      // non blocking
  lcdLoop();        // non blocking
  Logger.loop();    // non blocking
  webSocket.loop(); // non blocking
  stackmat.loop();  // non blocking
  stackmatLoop();   // non blocking

  sleepDetection();

  delay(5);
}

void core2(void *pvParameters) {
  while (1) {
    loop2();
  }
}

unsigned long lastBatRead = 0;
inline void loop2() {
  if (update) return; // return if update'ing

  rfidLoop();     // blocking (when card is close to scanner)
  buttons.loop(); // blocking

  if (millis() - lastBatRead > BATTERY_READ_INTERVAL) {
    currentBatteryVoltage = readBatteryVoltage(BAT_ADC, 15);
    float batPerct = voltageToPercentage(currentBatteryVoltage);

    sendBatteryStats(batPerct, currentBatteryVoltage);
    lastBatRead = millis();
  }

  delay(10);
}

// TODO: maybe move it into own file?
unsigned long lastCardReadTime = 0;
unsigned long lastCardId = 0;
void rfidLoop() {
  if (millis() - lastCardReadTime < 500) return;
  if (!mfrc522.PICC_IsNewCardPresent()) return;
  if (!mfrc522.PICC_ReadCardSerial()) return;

  unsigned long cardId = mfrc522.uid.uidByte[0] + (mfrc522.uid.uidByte[1] << 8) + (mfrc522.uid.uidByte[2] << 16) + (mfrc522.uid.uidByte[3] << 24);
  if (lastCardId == cardId && millis() - lastCardReadTime < 2500) return; // if same as last card (in 2.5s)

  Logger.printf("Scanned card ID: %lu\n", cardId);
  scanCard(cardId);
  lastCardId = cardId;

  mfrc522.PICC_HaltA();
  lastCardReadTime = millis();
}

void sleepDetection() {
  unsigned long timeSinceLastDraw = millis() - lcdLastDraw;
  if (timeSinceLastDraw > SLEEP_TIME && !lcdHasChanged && !stackmat.connected() && !state.testMode) {
    lcdPrintf(0, true, ALIGN_CENTER, "Sleep mode");
    lcdPrintf(1, true, ALIGN_CENTER, "Turn on timer");
    lcd.noBacklight();
    mfrc522.PCD_SoftPowerDown();

    // enter light sleep and wait for SLEEP_WAKE_BUTTON to be pressed
    lightSleep(SLEEP_WAKE_BUTTON, LOW);

    lcd.backlight();
    lcdClear();
    stateHasChanged = true;

    WiFi.disconnect();
    WiFi.reconnect();
    mfrc522.PCD_SoftPowerUp();

    return;
  }
}

void stackmatLoop() {
  StackmatTimerState stackmatState = stackmat.state();

  if (stackmatState != state.lastTimerState && stackmatState != ST_Unknown) {
    Logger.printf("Stackmat state change to: %d\n", stackmatState);
    
    switch (stackmatState) {
      case ST_Stopped:
        if (state.solveTime > 0) break;
        if (state.competitorCardId == 0) {
          state.currentScene = SCENE_WAITING_FOR_COMPETITOR_WITH_TIME;
          break;
        }

        Logger.printf("FINISH! Final time is %i:%02i.%03i!\n", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
        startSolveSession(stackmat.time());
        break;

      case ST_Reset:
        if(state.competitorCardId == 0 && (state.currentScene == SCENE_TIMER_TIME || state.currentScene == SCENE_WAITING_FOR_COMPETITOR_WITH_TIME)) {
          resetSolveState();
        }

        Logger.println("Timer reset!");
        break;

      case ST_Running:
        if (state.solveTime > 0) break;
        if (state.useInspection) stopInspection();
        // if (state.competitorCardId == 0) break;

        state.currentScene = SCENE_TIMER_TIME;
        Logger.println("Solve started!");
        break;

      default:
        break;
    }

    stateHasChanged = true;
  }

  if (stackmatState == StackmatTimerState::ST_Running && state.currentScene == SCENE_TIMER_TIME) {
    lcdPrintf(0, true, ALIGN_CENTER, "%s", displayTime(stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds()).c_str());
    displayStr(displayTime(stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds(), false));
    lcdClearLine(1);
  }

  if(stackmatState != ST_Unknown) state.lastTimerState = stackmatState;
}
