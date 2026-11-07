# The vendored `time` (#50), run under TZ=Asia/Kolkata by tests/time.rs.
require 'time'

def show(label)
  print label, ": "
  p yield
rescue => e
  puts "#{e.class}: #{e.message}"
end

t = Time.utc(2024, 2, 29, 13, 5, 9, 123456)
show("iso8601") { t.iso8601 }
show("xmlschema 3") { t.xmlschema(3) }
show("rfc2822") { t.rfc2822 }
show("httpdate") { t.httpdate }
show("rfc822") { t.rfc822 }
show("local xmlschema") { Time.local(2024, 7, 4, 1, 2, 3).xmlschema }
show("local rfc2822") { Time.local(2024, 7, 4).rfc2822 }
show("parse offset") { Time.parse("2024-02-29 13:05:09 +0900") }
show("parse rfc") { Time.parse("Thu, 29 Feb 2024 13:05:09 GMT") }
show("parse now") { Time.parse("12:30", Time.utc(2020, 1, 1)) }
show("parse local") { Time.parse("2024-12-01 10:00").to_s }
show("parse local offset") { Time.parse("2024-06-01 10:00").utc_offset }
show("parse usec") { Time.parse("2024-02-29T13:05:09.5-08:00").usec }
show("parse to_a") { Time.parse("Feb 3 2001").to_a }
show("parse EST") { Time.parse("2024-06-01 10:00 EST") }
show("parse PDT") { Time.parse("2024-06-01 10:00 PDT").utc_offset }
show("parse leap second") { Time.parse("1999-12-31 23:59:60 UTC") }
show("parse utc?") { Time.parse("10:00 +0000").utc? }
show("parse garbage") { Time.parse("garbage") }
show("strptime z") { Time.strptime("2024-03-01 10:11:12 +0100", "%Y-%m-%d %H:%M:%S %z") }
show("strptime s") { Time.strptime("1700000000", "%s") }
show("strptime dmy") { Time.strptime("29/02/2024", "%d/%m/%Y") }
show("strptime pm") { Time.strptime("07/04/24 01:02 PM", "%m/%d/%y %I:%M %p") }
show("strptime j") { Time.strptime("2024 186", "%Y %j") }
show("strptime N") { Time.strptime("2001-02-03T04:05:06.789+07:00", "%Y-%m-%dT%H:%M:%S.%N%z").nsec }
show("strptime bad") { Time.strptime("x", "%Y") }
show("rfc2822 parse") { Time.rfc2822("Thu, 29 Feb 2024 13:05:09 -0500") }
show("rfc2822 UT") { Time.rfc2822("Sun, 1 Jan 2023 00:00:00 UT") }
show("rfc2822 short") { Time.rfc2822("1 Jan 23 00:00 EST") }
show("httpdate parse") { Time.httpdate("Thu, 29 Feb 2024 13:05:09 GMT") }
show("httpdate bad") { Time.httpdate("nope") }
show("xmlschema parse") { Time.xmlschema("2024-02-29T13:05:09.25+02:00") }
show("xmlschema date") { Time.xmlschema("2024-07-04") }
show("iso8601 parse") { Time.iso8601("2024-02-29T13:05:09Z") }
show("iso8601 bad") { Time.iso8601("2024") }
show("zone_offset") { [Time.zone_offset("EST"), Time.zone_offset("+05:30"), Time.zone_offset("XYZ")] }
show("to_date") { Time.parse("2024-01-01").to_date }
show("to_datetime") { Time.utc(2024, 1, 1).to_datetime }
show("to_time") { t.to_time.equal?(t) }
