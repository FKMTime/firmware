#include <Arduino.h>

const unsigned long BIT_DURATION = 1000000 / 1200; // baud rate 1200

void setup() {
  Serial.begin(115200);
  pinMode(A0, INPUT);
}

unsigned long lastBitTime = 0;
void loop() {
  unsigned long curr = micros() ;
  if (curr - lastBitTime >= BIT_DURATION) {
    int rxVal = analogRead(A0);
    bool bit = rxVal > 1000 ? HIGH : LOW;
    Serial.print(bit);

    lastBitTime = curr; 
  }
}
