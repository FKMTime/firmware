#include <Arduino.h>


#include <SoftwareSerial.h>

SoftwareSerial swSer(14, 12, false, 128);

const unsigned long BIT_DURATION = 1000000 / 1200; // baud rate 1200

void setup() {
  Serial.begin(115200);
  // pinMode(A0, INPUT);
}

// unsigned long lastBitTime = 0;
// byte receivedByte = 0;
// int bitCounter = 0;
void loop() {
  // unsigned long curr = micros();
  // if (curr - lastBitTime >= BIT_DURATION) {
  //   int rxVal = analogRead(A0);
  //   bool bit = rxVal > 1000 ? LOW : HIGH;
  //   Serial.print(bit);
    
  //   lastBitTime = curr;
  // }
}
