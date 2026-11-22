#pragma listing on "..\listing.dir"
_Pragma ( "listing on \"..\\listing.dir\"" )
#define LISTING(x) PRAGMA(listing on #x)
#define PRAGMA(x) _Pragma(#x)
LISTING ( ..\listing.dir )
