#include <Arduino.h>
#include <EEPROM.h>
#include <stackmat.h>

struct GlobalState {
  // TIMER INTERNALS
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  bool timeConfirmed;
  unsigned long solverCardId;
  String solverName;

  // STACKMAT
  StackmatTimerState lastTiemrState;
  bool stackmatConnected;

  // RFID
  unsigned long lastCardReadTime;
};

String getChipID() {
  uint64_t chipid = ESP.getChipId();
  String chipidStr = String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
  return chipidStr;
}

void writeEEPROMInt(int address, int value) {
  byte lowByte = (value & 0xFF);
  byte highByte = ((value >> 8) & 0xFF);

  EEPROM.write(address, lowByte);
  EEPROM.write(address + 1, highByte);
}

int readEEPROMInt(int address) {
  byte lowByte = EEPROM.read(address);
  byte highByte = EEPROM.read(address + 1);

  return (lowByte | (highByte << 8));
}